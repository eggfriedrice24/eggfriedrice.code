//! What an error answer of the API means, and whether to send the request again.
//!
//! The provider sends a request through `efr_http::RetryPolicy::run`. For a status
//! that is not a success, one attempt reads the small error body, `{"type": "error",
//! "error": {"type", "message", "details"?}, "request_id"}`, and classifies it with
//! [`answer`] before the policy decides, so a spend cap is never sent four times.
//! `efr_http::is_retryable_status` does not decide here, and no Anthropic rule goes
//! into `efr-http`. The classes:
//!
//! | Answer | `ProviderError` | Again |
//! |---|---|---|
//! | 400 `invalid_request_error` whose message starts `prompt is too long` | `ContextOverflow` | no; the conversation compacts |
//! | 400 whose message starts `You have reached your specified API usage limits` | `Api` | no |
//! | any other 400, such as a missing `anthropic-workspace-id` | `Api` with the server's message | no |
//! | 401 `authentication_error` | `Unauthorized { message }`, at once (a key cannot refresh) | no |
//! | 402 `billing_error`, 403 `permission_error` | `Api` | no |
//! | 404 `not_found_error` for the model | `UnknownModel` | no |
//! | 413 `request_too_large` (a body over 32 MB) | `Api`, built here and not through `ProviderError::api`, which reads 413 as an overflow | no |
//! | 429 with `error.details.error_code: "enforced_spend_limit_reached"` | `Api` | no |
//! | any other 429 | `RateLimited { retry_after }` from `retry-after` | yes, after that wait |
//! | 500 `api_error`, 504 `timeout_error`, and 502 or 503 from a proxy | `Api` after the last try | yes, with backoff |
//! | 529 `overloaded_error` | `Overloaded` after the last try | yes, with backoff |
//! | any other status | `Api` with the server's message | no |
//! | an `error` event after a 200 | the same class by its `error.type` ([`event`]) | never: the stream has started |
//!
//! A 5xx may come after the server ran the request, but a Messages call keeps no state
//! on the server and its stream has not started, so a second copy is safe. A failure
//! to send is tried again only when no connection was made, because the server never
//! saw the request. Error messages come only from the server's error body, never from
//! the request, so they never hold the key.

use std::time::Duration;

use efr_http::{HttpError, Outcome, Retryable, StatusCode};
use efr_provider::ProviderError;
use jiff::Timestamp;
use serde_json::Value;

/// The longest error message kept from a response body, in characters.
const MAX_ERROR_MESSAGE: usize = 1000;

/// The `error.details.error_code` of a 429 for a spend cap of the account's tier: it
/// fails again until the cap resets, so waiting does not help.
const SPEND_LIMIT_CODE: &str = "enforced_spend_limit_reached";

/// The `error.type` of a 404 for a model that the API does not serve to the key.
const NOT_FOUND: &str = "not_found_error";

/// Anthropic's status for an overloaded API, which `http` has no name for.
const OVERLOADED: u16 = 529;

/// What one failed attempt means: the error, and whether to send the request again.
#[derive(Debug)]
pub(crate) struct Failure {
    /// The error, which the call returns when the policy stops.
    pub(crate) error: ProviderError,
    /// Whether another attempt may pass.
    pub(crate) again: Again,
}

/// Whether to send a failed request again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Again {
    /// Another attempt fails the same way.
    Never,
    /// Another attempt may pass: after the wait the server asked for, else after the
    /// policy's backoff.
    After(Option<Duration>),
}

impl Failure {
    /// The failure of an attempt that got no answer: tried again only when no
    /// connection was made, because a `POST` that may have reached the server must not
    /// run twice.
    pub(crate) fn transport(error: HttpError) -> Failure {
        let again = if error.is_transient() { Again::After(None) } else { Again::Never };
        Failure { error: transport(error), again }
    }
}

impl Retryable for Failure {
    fn outcome(&self, _now: Timestamp) -> Outcome {
        match self.again {
            Again::Never => Outcome::Final,
            Again::After(retry_after) => Outcome::Transient { retry_after },
        }
    }
}

/// The error of an HTTP failure.
pub(crate) fn transport(error: HttpError) -> ProviderError {
    ProviderError::Transport { source: Box::new(error) }
}

/// The failure of an error answer with `status` and `body`. `retry_after` is the wait
/// that its headers ask for, and `model` the model of a model call, for
/// `UnknownModel`; a fetch of the model list passes `None`.
pub(crate) fn answer(
    status: StatusCode,
    retry_after: Option<Duration>,
    body: &str,
    model: Option<&str>,
) -> Failure {
    let details = Details::parse(body);
    let class = Class::of(status.as_u16(), &details);
    let message = message_of(&details, status, body);
    let error = class.error(Some(status.as_u16()), details, message, retry_after, model);
    Failure { error, again: class.again(retry_after) }
}

/// The message of an error answer: the server's `error.message`, else its body as
/// text, else the status's reason.
fn message_of(details: &Details, status: StatusCode, body: &str) -> String {
    details.message.clone().unwrap_or_else(|| {
        let shown = clip(body.trim());
        if shown.is_empty() {
            status.canonical_reason().unwrap_or("no message").to_owned()
        } else {
            shown
        }
    })
}

/// The error of a refused key check: `Unauthorized` with the server's message for a
/// 401, else an `Api` error with the status and the server's message, such as a 403
/// or a 400 that asks for `anthropic-workspace-id`.
pub(crate) fn refusal(status: StatusCode, body: &str) -> ProviderError {
    let details = Details::parse(body);
    if status == StatusCode::UNAUTHORIZED {
        return ProviderError::Unauthorized { message: details.message };
    }
    let message = message_of(&details, status, body);
    ProviderError::Api { status: Some(status.as_u16()), code: details.kind, message }
}

/// The error of an `error` event in a stream that has started: the class of its
/// `error.type`, never sent again. `event` is the event's whole JSON data.
pub(crate) fn event(event: &Value) -> ProviderError {
    let details = Details::of(event);
    let class = details
        .kind
        .as_deref()
        .and_then(status_of_kind)
        .map_or(Class::Refused, |status| Class::of(status, &details));
    let message = details.message.clone().unwrap_or_else(|| "no message".to_owned());
    class.error(None, details, message, None, None)
}

/// The status that the API gives an `error.type`, for an error that arrives in a
/// stream without one.
fn status_of_kind(kind: &str) -> Option<u16> {
    Some(match kind {
        "invalid_request_error" => 400,
        "authentication_error" => 401,
        "billing_error" => 402,
        "permission_error" => 403,
        NOT_FOUND => 404,
        "request_too_large" => 413,
        "rate_limit_error" => 429,
        "api_error" => 500,
        "timeout_error" => 504,
        "overloaded_error" => OVERLOADED,
        _ => return None,
    })
}

/// What an error answer says, by status and body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// The API refused the request for what it holds: a 400 (an overflow when the
    /// message says so), 402, 403 and any status without a class of its own.
    Refused,
    /// The key was refused.
    Unauthorized,
    /// The API does not serve the model to this key.
    UnknownModel,
    /// The body is larger than the API takes. It is not an overflow of the window.
    TooLarge,
    /// The account's tier reached its spend cap.
    SpendCap,
    /// The account sends too fast.
    RateLimited,
    /// The API failed; another copy may pass.
    ServerError,
    /// The API is overloaded; another copy may pass.
    Overloaded,
}

impl Class {
    fn of(status: u16, details: &Details) -> Class {
        match status {
            401 => Class::Unauthorized,
            404 if details.kind.as_deref() == Some(NOT_FOUND) => Class::UnknownModel,
            413 => Class::TooLarge,
            429 if details.error_code.as_deref() == Some(SPEND_LIMIT_CODE) => Class::SpendCap,
            429 => Class::RateLimited,
            500 | 502 | 503 | 504 => Class::ServerError,
            OVERLOADED => Class::Overloaded,
            _ => Class::Refused,
        }
    }

    fn again(self, retry_after: Option<Duration>) -> Again {
        match self {
            Class::RateLimited | Class::ServerError | Class::Overloaded => {
                Again::After(retry_after)
            }
            _ => Again::Never,
        }
    }

    fn error(
        self,
        status: Option<u16>,
        details: Details,
        message: String,
        retry_after: Option<Duration>,
        model: Option<&str>,
    ) -> ProviderError {
        let code = details.kind;
        match (self, model) {
            (Class::Unauthorized, _) => ProviderError::Unauthorized { message: details.message },
            (Class::UnknownModel, Some(model)) => {
                ProviderError::UnknownModel { model: model.to_owned() }
            }
            (Class::RateLimited, _) => ProviderError::RateLimited { retry_after },
            (Class::Overloaded, _) => ProviderError::Overloaded { status, message },
            (Class::SpendCap, _) => {
                ProviderError::Api { status, code: details.error_code.or(code), message }
            }
            // NOTE: ProviderError::api reads a 413 as an overflow of the window, but on
            // this API it is a body over 32 MB that compaction may not shrink.
            (Class::TooLarge | Class::ServerError | Class::UnknownModel, _) => {
                ProviderError::Api { status, code, message }
            }
            (Class::Refused, _) => ProviderError::api(status, code, message),
        }
    }
}

/// The members of an error body that the classes read.
#[derive(Debug, Default)]
struct Details {
    /// `error.type`.
    kind: Option<String>,
    /// `error.message`, clipped.
    message: Option<String>,
    /// `error.details.error_code`.
    error_code: Option<String>,
}

impl Details {
    fn parse(body: &str) -> Details {
        match serde_json::from_str::<Value>(body) {
            Ok(value) => Details::of(&value),
            Err(_) => Details::default(),
        }
    }

    fn of(value: &Value) -> Details {
        let Some(error) = value.get("error") else {
            return Details::default();
        };
        let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
        Details {
            kind: text(error.get("type")),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .filter(|message| !message.trim().is_empty())
                .map(clip),
            error_code: text(error.get("details").and_then(|details| details.get("error_code"))),
        }
    }
}

fn clip(message: &str) -> String {
    message.chars().take(MAX_ERROR_MESSAGE).collect()
}

#[cfg(test)]
mod tests;
