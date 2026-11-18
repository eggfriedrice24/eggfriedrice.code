//! The typed client of the Expo push service: the message, the ticket, the receipt and
//! their codes, as `localapi` is for tailscaled.
//!
//! efrd sends a push as JSON to the service, which sends it to Apple with the push key
//! that EAS holds for the app (ADR 0028). [`ExpoPush::send`] posts at most
//! [`MAX_MESSAGES`] messages and gets one [`Ticket`] for each, in their order;
//! [`ExpoPush::receipts`] asks for the [`Receipt`] of at most [`MAX_RECEIPT_IDS`]
//! tickets. Each request has no gzip and no `accept-encoding`, so the answer is plain
//! JSON, and no access token: the "enhanced push security" option of the Expo account
//! must be off.
//!
//! A push token is a secret, and the `message` text of an answer can hold one. So the
//! parse reads only `status`, `id`, `details.error` and `errors[].code`, keeps a code
//! only when it is 1 to [`CODE_MAX_LEN`] characters of `[A-Za-z_]` ([`PushCode`]) and a
//! ticket id only when it is 1 to [`CODE_MAX_LEN`] characters of `[A-Za-z0-9-]`
//! ([`TicketId`]), and drops every other value. No request is marked
//! [`recorded`](crate::HttpRequest::recorded), so a recorder never sees a token.
//!
//! Each try of a request ends after [`REQUEST_TIMEOUT`] on the injected clock. A
//! request goes again after 429, a 5xx, a timeout or no connection ([`EXPO_RETRY`]); a
//! second copy of a send only replaces the first notice on the phone, by its collapse
//! id, so the requests are marked idempotent. No try of a send starts after the
//! smallest `ttl` of its messages. What efrd does with a code is the daemon's decision,
//! not this module's.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use http::header::ACCEPT;
use http::{HeaderValue, StatusCode};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};

use crate::retry::HttpAttempt;
use crate::{HttpClient, HttpError, HttpRequest, HttpResponse, RetryPolicy, redact};

/// The base URL of the Expo push service.
pub const EXPO_PUSH_BASE: &str = "https://exp.host";

/// The path of a send, below the base URL.
pub const SEND_PATH: &str = "/--/api/v2/push/send";

/// The path of a receipts request, below the base URL.
pub const RECEIPTS_PATH: &str = "/--/api/v2/push/getReceipts";

/// The most messages of one send: the limit of Expo.
pub const MAX_MESSAGES: usize = 100;

/// The most ticket ids of one receipts request: the limit of Expo.
pub const MAX_RECEIPT_IDS: usize = 1000;

/// How long one try of a request may take. The read timeout of the shared client is
/// for model streams and much longer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest code and ticket id that the parse keeps.
pub const CODE_MAX_LEN: usize = 64;

/// The title of every notice. The body says the rest.
pub const TITLE: &str = "efr";

/// The code of a token that Apple or Expo no longer knows: push is off for it.
pub const DEVICE_NOT_REGISTERED: &str = "DeviceNotRegistered";

/// The code of a project that has no valid push key at Expo.
pub const INVALID_CREDENTIALS: &str = "InvalidCredentials";

/// The code of a send that held tokens of two Expo projects.
pub const TOO_MANY_EXPERIENCE_IDS: &str = "PUSH_TOO_MANY_EXPERIENCE_IDS";

/// Four tries, with waits of up to 2 s, 4 s and 8 s, and a `Retry-After` of up to 60 s.
pub const EXPO_RETRY: RetryPolicy = RetryPolicy {
    max_attempts: 4,
    initial_backoff: Duration::from_secs(2),
    max_backoff: Duration::from_secs(8),
    max_retry_after: Duration::from_secs(60),
};

/// One notice for one phone.
#[derive(Debug, Clone)]
pub struct PushMessage {
    to: SecretString,
    body: String,
    ttl: Duration,
    conversation: Option<String>,
    daemon: String,
}

impl PushMessage {
    /// A notice for the phone of `token`, its Expo push token (`ExponentPushToken[...]`),
    /// with `body` under the title [`TITLE`]. Apple keeps it `ttl` for a phone that is
    /// off, and no try of its send starts after that. A tap opens `conversation` on the
    /// efrd `daemon`; a newer notice of the same conversation replaces the older one,
    /// and iOS groups them.
    pub fn new(
        token: &str,
        body: String,
        ttl: Duration,
        conversation: Option<String>,
        daemon: String,
    ) -> Self {
        PushMessage { to: SecretString::from(token), body, ttl, conversation, daemon }
    }
}

/// A code of the Expo push service, such as [`DEVICE_NOT_REGISTERED`]: 1 to
/// [`CODE_MAX_LEN`] characters of `[A-Za-z_]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PushCode(String);

impl PushCode {
    /// The code in `text`, or `None` when it is not of the form of a code.
    pub fn new(text: &str) -> Option<Self> {
        let valid = (1..=CODE_MAX_LEN).contains(&text.len())
            && text.bytes().all(|byte| byte.is_ascii_alphabetic() || byte == b'_');
        valid.then(|| PushCode(text.to_owned()))
    }

    /// The code.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The id of a ticket, with which efrd asks for its receipt: 1 to [`CODE_MAX_LEN`]
/// characters of `[A-Za-z0-9-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TicketId(String);

impl TicketId {
    /// The id in `text`, or `None` when it is not of the form of an id.
    pub fn new(text: &str) -> Option<Self> {
        let valid = (1..=CODE_MAX_LEN).contains(&text.len())
            && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        valid.then(|| TicketId(text.to_owned()))
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the Expo push service did with one message.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ticket {
    /// The service took the message; its receipt comes later.
    Ok(TicketId),
    /// The service refused the message, with its code when it gave one that efrd
    /// keeps.
    Failed(Option<PushCode>),
}

/// What Apple did with one notice, as its receipt says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Receipt {
    /// Apple took the notice.
    Ok,
    /// The notice did not reach Apple, with its code when the service gave one that
    /// efrd keeps.
    Failed(Option<PushCode>),
}

/// The answer to a whole request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExpoAnswer<T> {
    /// The service handled the request.
    Data(T),
    /// The service refused the request as a whole: an `errors` array, or a status that
    /// is not a success, after the tries of [`EXPO_RETRY`].
    Refused {
        /// The status of the last answer.
        status: StatusCode,
        /// The first code of its `errors` that efrd keeps.
        code: Option<PushCode>,
    },
}

/// A client of the Expo push service at one base URL.
#[derive(Debug, Clone)]
pub struct ExpoPush {
    send: HttpRequest,
    receipts: HttpRequest,
    client: HttpClient,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
}

impl ExpoPush {
    /// A client of the service at `base`, such as [`EXPO_PUSH_BASE`] or a test's fake,
    /// over `client`. Its tries wait on `clock` and draw their jitter from `rng`. Fails
    /// for a base that is not an absolute `http` or `https` URL.
    pub fn new(
        base: &str,
        client: HttpClient,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Result<Self, HttpError> {
        let base = base.trim_end_matches('/');
        let template = |path: &str| -> Result<HttpRequest, HttpError> {
            let request = HttpRequest::post(&format!("{base}{path}"))?;
            Ok(request.header(ACCEPT, HeaderValue::from_static("application/json")).idempotent())
        };
        Ok(ExpoPush {
            send: template(SEND_PATH)?,
            receipts: template(RECEIPTS_PATH)?,
            client,
            clock,
            rng,
        })
    }

    /// Sends `messages`, at most [`MAX_MESSAGES`], in one request: one ticket for each
    /// message that the answer holds, in their order. Fails when no answer came after
    /// the tries, or when the answer is not the JSON of a send.
    pub async fn send(
        &self,
        messages: &[PushMessage],
    ) -> Result<ExpoAnswer<Vec<Ticket>>, HttpError> {
        let wire: Vec<WireMessage<'_>> = messages.iter().map(WireMessage::of).collect();
        let request = self.send.clone().json(&wire)?;
        let ttl = messages.iter().map(|message| message.ttl).min();
        let response = self.exchange(&request, ttl).await?;
        let answer = self.answer::<Vec<WireTicket>>(response).await?;
        Ok(map(answer, |tickets| tickets.into_iter().map(WireTicket::ticket).collect()))
    }

    /// Asks for the receipts of `ids`, at most [`MAX_RECEIPT_IDS`]: each receipt that
    /// the answer holds, with its ticket id. A ticket with no receipt yet is not in it.
    pub async fn receipts(
        &self,
        ids: &[TicketId],
    ) -> Result<ExpoAnswer<Vec<(TicketId, Receipt)>>, HttpError> {
        let ids: Vec<&str> = ids.iter().map(TicketId::as_str).collect();
        let request = self.receipts.clone().json(&WireIds { ids })?;
        let response = self.exchange(&request, None).await?;
        let answer = self.answer::<HashMap<String, WireTicket>>(response).await?;
        Ok(map(answer, |receipts| {
            receipts
                .into_iter()
                .filter_map(|(id, receipt)| Some((TicketId::new(&id)?, receipt.receipt())))
                .collect()
        }))
    }

    /// Sends `request` with the tries of [`EXPO_RETRY`], each one at most [`REQUEST_TIMEOUT`]
    /// on the clock. No try after the first starts once `ttl` passed.
    async fn exchange(
        &self,
        request: &HttpRequest,
        ttl: Option<Duration>,
    ) -> Result<HttpResponse, HttpError> {
        let started = self.clock.now();
        let url = redact::url(request.url());
        let attempt = |attempt: u32| {
            let url = url.clone();
            async move {
                let late = ttl.is_some_and(|ttl| {
                    let since = self.clock.now().duration_since(started);
                    Duration::try_from(since).unwrap_or_default() >= ttl
                });
                if attempt > 1 && late {
                    return Err(HttpAttempt {
                        value: HttpError::Timeout { url },
                        idempotent: false,
                    });
                }
                match self.clock.timeout(REQUEST_TIMEOUT, self.client.send(request)).await {
                    Ok(Ok(value)) => Ok(HttpAttempt { value, idempotent: true }),
                    Ok(Err(value)) => Err(HttpAttempt { value, idempotent: true }),
                    Err(_) => {
                        Err(HttpAttempt { value: HttpError::Timeout { url }, idempotent: true })
                    }
                }
            }
        };
        EXPO_RETRY
            .run(&*self.clock, &*self.rng, attempt)
            .await
            .map(|attempt| attempt.value)
            .map_err(|attempt| attempt.value)
    }

    /// The answer of `response`, with its body read within [`REQUEST_TIMEOUT`] on the
    /// clock: a body that stalls after its head ends the request too.
    async fn answer<T: serde::de::DeserializeOwned>(
        &self,
        response: HttpResponse,
    ) -> Result<ExpoAnswer<T>, HttpError> {
        let url = response.url().to_owned();
        let read = self.clock.timeout(REQUEST_TIMEOUT, parse(response)).await;
        read.unwrap_or_else(|_| Err(HttpError::Timeout { url }))
    }
}

/// `answer` with its data changed by `f`.
fn map<T, U>(answer: ExpoAnswer<T>, f: impl FnOnce(T) -> U) -> ExpoAnswer<U> {
    match answer {
        ExpoAnswer::Data(data) => ExpoAnswer::Data(f(data)),
        ExpoAnswer::Refused { status, code } => ExpoAnswer::Refused { status, code },
    }
}

/// The answer of `response`: its data, or the refusal of the whole request. The body of
/// a status that is not a success need not be JSON.
async fn parse<T: serde::de::DeserializeOwned>(
    response: HttpResponse,
) -> Result<ExpoAnswer<T>, HttpError> {
    let status = response.status();
    if !status.is_success() {
        let bytes = response.bytes().await?;
        let errors = serde_json::from_slice::<WireBody<serde::de::IgnoredAny>>(&bytes)
            .map(|body| body.errors)
            .unwrap_or_default();
        return Ok(ExpoAnswer::Refused { status, code: first_code(&errors) });
    }
    let body = response.json::<WireBody<T>>().await?;
    match body.data {
        Some(data) if body.errors.is_empty() => Ok(ExpoAnswer::Data(data)),
        _ => Ok(ExpoAnswer::Refused { status, code: first_code(&body.errors) }),
    }
}

/// The first code of `errors` that is of the form of a code.
fn first_code(errors: &[WireError]) -> Option<PushCode> {
    errors.iter().find_map(|error| error.code.as_deref().and_then(PushCode::new))
}

/// The JSON of one message.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireMessage<'a> {
    to: &'a str,
    title: &'static str,
    body: &'a str,
    sound: &'static str,
    priority: &'static str,
    ttl: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    collapse_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<&'a str>,
    data: WireData<'a>,
}

impl<'a> WireMessage<'a> {
    fn of(message: &'a PushMessage) -> Self {
        let conversation = message.conversation.as_deref();
        WireMessage {
            to: message.to.expose_secret(),
            title: TITLE,
            body: &message.body,
            sound: "default",
            priority: "high",
            ttl: message.ttl.as_secs(),
            collapse_id: conversation,
            thread_id: conversation,
            data: WireData { daemon: &message.daemon, conversation },
        }
    }
}

/// The `data` of a message: the two ids that a tap needs, and nothing else.
#[derive(Serialize)]
struct WireData<'a> {
    #[serde(rename = "d")]
    daemon: &'a str,
    #[serde(rename = "c", skip_serializing_if = "Option::is_none")]
    conversation: Option<&'a str>,
}

/// The body of a receipts request.
#[derive(Serialize)]
struct WireIds<'a> {
    ids: Vec<&'a str>,
}

/// The body of an answer. Each other member, `message` among them, is skipped unread.
#[derive(Deserialize)]
struct WireBody<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<WireError>,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    code: Option<String>,
}

/// A ticket or a receipt: both have a `status`, and an error has its code in
/// `details.error`.
#[derive(Deserialize)]
struct WireTicket {
    status: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    details: Option<WireDetails>,
}

#[derive(Deserialize)]
struct WireDetails {
    #[serde(default)]
    error: Option<String>,
}

impl WireTicket {
    fn code(&self) -> Option<PushCode> {
        self.details.as_ref().and_then(|details| details.error.as_deref()).and_then(PushCode::new)
    }

    /// An `ok` ticket with an id that efrd keeps is taken; every other one failed.
    fn ticket(self) -> Ticket {
        match self.id.as_deref().and_then(TicketId::new) {
            Some(id) if self.status == "ok" => Ticket::Ok(id),
            _ => Ticket::Failed(self.code()),
        }
    }

    fn receipt(self) -> Receipt {
        if self.status == "ok" { Receipt::Ok } else { Receipt::Failed(self.code()) }
    }
}

#[cfg(test)]
mod tests;
