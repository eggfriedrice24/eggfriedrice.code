//! The one public error type of the crate.

use std::time::Duration;

/// Every way a model request can fail, for every provider.
///
/// Provider crates map their own failures onto these variants, so the conversation
/// decides what to do (refresh, back off, give up) without knowing the provider. A
/// token source reports through the same type, because its failures surface as a
/// failed request.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The provider rejected the credentials: at once for a token source that cannot
    /// refresh, such as an API key, else after the source was invalidated and asked
    /// once more.
    #[error("the provider rejected the credentials{}", server_text(.message.as_deref()))]
    Unauthorized {
        /// The provider's own message, when it sent one, such as a missing scope of a
        /// restricted key or an expired key. It comes from the server's error body and
        /// never from the request, so it never holds the credentials.
        message: Option<String>,
    },

    /// The provider is rate limiting requests.
    #[error("the provider is rate limiting requests{}", retry_hint(*.retry_after))]
    RateLimited {
        /// How long the provider asked the client to wait, when it said.
        retry_after: Option<Duration>,
    },

    /// The provider is overloaded and took no new request, after the provider's own
    /// retries, such as Anthropic's 529 `overloaded_error`. It is not a rate limit of
    /// the account: waiting a short time helps, and nothing in the request was wrong.
    #[error("the provider is overloaded; try again later")]
    Overloaded {
        /// The HTTP status, when the error came as a response rather than inside a
        /// stream.
        status: Option<u16>,
        /// The provider's message.
        message: String,
    },

    /// No credentials are stored for the provider, so there is no token to send.
    #[error("no credentials are stored for the provider")]
    NotLoggedIn,

    /// The token source could not produce an access token, for example because a
    /// refresh failed.
    #[error("the token source could not produce an access token")]
    Token {
        /// The token source's own error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// The request could not be sent, or the response could not be read.
    #[error("the request to the provider failed in transit")]
    Transport {
        /// The HTTP layer's error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// The provider answered with an error of its own.
    #[error("the provider answered with an error: {message}")]
    Api {
        /// The HTTP status, when the error came as a response rather than inside a
        /// stream.
        status: Option<u16>,
        /// The provider's machine-readable error code, when it sent one.
        code: Option<String>,
        /// The provider's message.
        message: String,
    },

    /// The request is larger than the model's context window. It is never transient:
    /// the same request fails again, so the conversation compacts its context before it
    /// sends another one. [`ProviderError::api`] picks this variant for an API error
    /// that says so.
    #[error("the request is larger than the model's context window")]
    ContextOverflow {
        /// The HTTP status, when the error came as a response rather than inside a
        /// stream.
        status: Option<u16>,
        /// The provider's machine-readable error code, when it sent one.
        code: Option<String>,
        /// The provider's message.
        message: String,
    },

    /// A response body or a stream event could not be parsed.
    #[error("the provider sent a response that could not be parsed")]
    Decode {
        /// The parser's error.
        #[source]
        source: serde_json::Error,
    },

    /// The provider does not serve the requested model.
    #[error("the provider does not serve the model {model:?}")]
    UnknownModel {
        /// The model id of the request.
        model: String,
    },

    /// The event stream broke the order that [`ProviderEvent`](crate::ProviderEvent)
    /// documents, such as arguments for a tool call that never started.
    #[error("the provider's event stream is malformed: {problem}")]
    InvalidStream {
        /// What was wrong.
        problem: &'static str,
    },

    /// The event stream ended before its `Done` event.
    #[error("the provider's event stream ended before the response was done")]
    Incomplete,

    /// A provider id breaks the naming rules of [`ProviderId`](crate::ProviderId).
    #[error("{id:?} is not a valid provider id")]
    InvalidProviderId {
        /// The rejected id.
        id: String,
    },
}

/// The error codes with which a provider refuses a request that does not fit in the
/// model's context window: OpenAI's Responses and Chat APIs.
const CONTEXT_OVERFLOW_CODES: &[&str] = &["context_length_exceeded"];

/// The start of the message with which Anthropic's Messages API refuses a request that
/// does not fit, inside an `invalid_request_error`.
const CONTEXT_OVERFLOW_MESSAGE: &str = "prompt is too long";

/// HTTP 413 Payload Too Large: the request body is larger than the backend takes.
const PAYLOAD_TOO_LARGE: u16 = 413;

impl ProviderError {
    /// The error for an error answer of a provider's API: [`ProviderError::ContextOverflow`]
    /// when the answer says that the request does not fit in the model's context window
    /// (the code `context_length_exceeded`, HTTP 413, or a message that starts with
    /// `prompt is too long`), else [`ProviderError::Api`]. The typed signals come first;
    /// the message is the last resort, for a provider without a code of its own.
    pub fn api(status: Option<u16>, code: Option<String>, message: String) -> ProviderError {
        let by_code = code.as_deref().is_some_and(|code| CONTEXT_OVERFLOW_CODES.contains(&code));
        let by_status = status == Some(PAYLOAD_TOO_LARGE);
        let by_message = message
            .trim_start()
            .get(..CONTEXT_OVERFLOW_MESSAGE.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(CONTEXT_OVERFLOW_MESSAGE));
        if by_code || by_status || by_message {
            ProviderError::ContextOverflow { status, code, message }
        } else {
            ProviderError::Api { status, code, message }
        }
    }

    /// True for [`ProviderError::ContextOverflow`].
    pub fn is_context_overflow(&self) -> bool {
        matches!(self, ProviderError::ContextOverflow { .. })
    }
}

/// The tail of a message that the server's own text completes.
fn server_text(message: Option<&str>) -> String {
    match message {
        Some(message) => format!(": {message}"),
        None => String::new(),
    }
}

/// The tail of the rate-limit message: the delay rounded up to whole seconds, so a
/// reader never retries early, in its two largest units. A plan's usage limit resets
/// hours later, and `10800s` is not a time a person reads at a glance.
fn retry_hint(retry_after: Option<Duration>) -> String {
    match retry_after {
        Some(delay) => {
            let seconds = delay.as_secs() + u64::from(delay.subsec_nanos() > 0);
            format!("; retry after {}", span(seconds))
        }
        None => String::new(),
    }
}

/// `seconds` as `45s`, `5m 3s`, `3h 0m` or `2d 4h`.
fn span(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, _) => format!("{minutes}m {}s", seconds % 60),
        (0, _, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

#[cfg(test)]
mod tests;
