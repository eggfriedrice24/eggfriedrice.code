//! A provider driven by a transcript.
//!
//! The provider records of a transcript form exchanges: one `expect_outbound`
//! `provider_request`, then the `emit_inbound` `provider_sse` records up to the next
//! request. Each call to [`Provider::stream`] takes the next exchange, checks the
//! request against it after redaction, and streams its answer. Records of other kinds
//! belong to the scenario driver and are skipped here.
//!
//! The answer format is canonical, because this provider stands in for every real
//! one: each `data` field of a `provider_sse` body is a [`ProviderEvent`] in its serde
//! form, `{"kind": "text_delta", "data": {"text": "..."}}`. An event of type `error`
//! ends the answer with a [`ProviderError`]; its data is one of
//! `{"kind": "unauthorized"}`, `{"kind": "rate_limited", "retry_after_ms": 1000}`,
//! `{"kind": "not_logged_in"}`, `{"kind": "incomplete"}` or
//! `{"kind": "api", "status": 500, "code": "server_error", "message": "..."}`
//! (`status` and `code` optional). An `api` error goes through [`ProviderError::api`],
//! as a real provider's does, so the code `context_length_exceeded` or the status 413
//! gives [`ProviderError::ContextOverflow`]. A provider's own wire format is replayed at
//! the HTTP level instead, with wiremock in front of the real client.

mod sse;

use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use efr_provider::{Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request};
use futures::StreamExt as _;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::watch;

use crate::{Inbound, Outbound, Record, Redactor, TestSupportError, Transcript};

/// The id a replay provider has unless told otherwise.
const DEFAULT_ID: &str = "replay";

/// A [`Provider`] that answers from a transcript and checks every request against it.
///
/// Each request is compared with the next `provider_request` record as JSON, after the
/// [`Redactor`] has replaced run-specific values on both sides; the answer's events have
/// their placeholders restored, so a fixture can say `<CWD>` and the code under test
/// sees the real directory. The transcript is validated when the provider is built, so
/// a broken fixture fails before the test runs.
///
/// A request that does not match, or that comes after the last exchange, fails with
/// [`ProviderError::Api`] (code `replay_mismatch` or `replay_exhausted`) instead of a
/// panic, because the code under test may run in a task whose panic the test would
/// never see. Every later request fails the same way. The test calls
/// [`finish`](Self::finish) at the end, which reports the first failure in full, or the
/// exchanges that were never requested.
///
/// [`paced`](Self::paced) holds each `provider_sse` record until the harness has handled
/// the records before it, for scenarios such as an interrupt in the middle of an
/// answer.
pub struct ReplayProvider {
    id: ProviderId,
    redactor: Redactor,
    state: Mutex<State>,
    /// The last transcript line the harness has handled, when the provider is paced.
    handled: Option<watch::Sender<usize>>,
}

struct State {
    exchanges: VecDeque<Exchange>,
    served: usize,
    failure: Option<Failure>,
}

/// One request and its answer.
struct Exchange {
    line: usize,
    expected: Value,
    pieces: Vec<Piece>,
}

/// The events of one `provider_sse` record.
struct Piece {
    /// The last line of another kind between the previous provider record of the
    /// exchange and this one; a paced provider waits until the harness has handled it.
    hold_until: Option<usize>,
    items: Vec<Item>,
}

/// One answer event, kept as JSON so that placeholders are restored when it is sent.
enum Item {
    Event(Value),
    Error(Value),
}

/// The first failed request, kept so that every later call and `finish` report it.
#[derive(Clone)]
enum Failure {
    Exhausted { request: Value },
    Mismatch { line: usize, pointer: String, expected: Value, actual: Value },
}

/// The data of an `error` event.
///
/// The variants without data are written with braces because serde enforces
/// `deny_unknown_fields` only on struct variants of an internally tagged enum.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ErrorRecord {
    Unauthorized {},
    RateLimited {
        #[serde(default)]
        retry_after_ms: Option<u64>,
    },
    NotLoggedIn {},
    Incomplete {},
    Api {
        #[serde(default)]
        status: Option<u16>,
        #[serde(default)]
        code: Option<String>,
        message: String,
    },
}

impl ReplayProvider {
    /// The `code` of the [`ProviderError::Api`] that a request gets when it does not
    /// match its exchange.
    pub const MISMATCH_CODE: &'static str = "replay_mismatch";

    /// The `code` of the [`ProviderError::Api`] that a request gets when the transcript
    /// has no exchange left.
    pub const EXHAUSTED_CODE: &'static str = "replay_exhausted";

    /// A provider with the id `replay` over the provider records of `transcript`, with
    /// a redactor that replaces only timestamps. Fails when a provider record is
    /// invalid: a `provider_sse` before any request, a request body that is not an
    /// `efr_provider::Request`, or an answer that is not canonical server-sent events.
    pub fn new(transcript: &Transcript) -> Result<Self, TestSupportError> {
        let state = State { exchanges: exchanges(transcript)?, served: 0, failure: None };
        Ok(ReplayProvider {
            id: replay_id(),
            redactor: Redactor::new(),
            state: Mutex::new(state),
            handled: None,
        })
    }

    /// The same provider with another id.
    #[must_use]
    pub fn with_id(mut self, id: ProviderId) -> Self {
        self.id = id;
        self
    }

    /// The same provider with `redactor` for comparing requests and restoring answers,
    /// normally [`TestDirs::redactor`](crate::TestDirs::redactor) with the test's
    /// working directory added.
    #[must_use]
    pub fn with_redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// The same provider, paced: the events of a `provider_sse` record are held until
    /// [`handled_through`](Self::handled_through) reaches the last record of another
    /// kind that comes before it in the exchange. A record with nothing else before it
    /// is never held.
    #[must_use]
    pub fn paced(mut self) -> Self {
        self.handled = Some(watch::channel(0).0);
        self
    }

    /// Tells a paced provider that the harness has handled every record up to and
    /// including `line`, which releases the answers held until then. A provider that is
    /// not paced ignores it.
    pub fn handled_through(&self, line: usize) {
        if let Some(handled) = &self.handled {
            handled.send_modify(|reached| *reached = (*reached).max(line));
        }
    }

    /// The number of requests that matched their exchange.
    pub fn served(&self) -> usize {
        self.lock().served
    }

    /// The number of exchanges not requested yet.
    pub fn remaining(&self) -> usize {
        self.lock().exchanges.len()
    }

    /// Checks that the replay went as the transcript says: no request failed and every
    /// exchange was requested.
    pub fn finish(&self) -> Result<(), TestSupportError> {
        let state = self.lock();
        if let Some(failure) = &state.failure {
            return Err(failure.to_error());
        }
        match state.exchanges.front() {
            Some(next) => Err(TestSupportError::UnusedRequests {
                served: state.served,
                remaining: state.exchanges.len(),
                next_line: next.line,
            }),
            None => Ok(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Nothing panics while it holds the lock, so a poisoned lock is still usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the next exchange for `request`, or records why the request failed.
    fn next_exchange(&self, request: &Request) -> Result<Exchange, ProviderError> {
        let actual = match serde_json::to_value(request) {
            Ok(actual) => self.redactor.redact_json(&actual),
            // NOTE: a Request always serializes, since its maps have string keys; this
            // arm only keeps the code free of a panic.
            Err(source) => return Err(ProviderError::Decode { source }),
        };
        let mut state = self.lock();
        if let Some(failure) = &state.failure {
            return Err(failure.to_provider_error());
        }
        let failure = match state.exchanges.pop_front() {
            None => Failure::Exhausted { request: actual },
            Some(exchange) => {
                let expected = self.redactor.redact_json(&exchange.expected);
                if expected == actual {
                    state.served += 1;
                    return Ok(exchange);
                }
                let pointer = first_difference(&expected, &actual);
                Failure::Mismatch { line: exchange.line, pointer, expected, actual }
            }
        };
        let error = failure.to_provider_error();
        state.failure = Some(failure);
        Err(error)
    }

    fn item(&self, item: Item) -> Result<ProviderEvent, ProviderError> {
        match item {
            Item::Event(data) => serde_json::from_value(self.redactor.restore_json(&data))
                .map_err(|source| ProviderError::Decode { source }),
            Item::Error(data) => {
                match serde_json::from_value::<ErrorRecord>(self.redactor.restore_json(&data)) {
                    Ok(record) => Err(record.into_error()),
                    Err(source) => Err(ProviderError::Decode { source }),
                }
            }
        }
    }
}

#[async_trait]
impl Provider for ReplayProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let exchange = self.next_exchange(&request)?;
        let pieces: Vec<(Option<usize>, Vec<Result<ProviderEvent, ProviderError>>)> = exchange
            .pieces
            .into_iter()
            .map(|piece| {
                (piece.hold_until, piece.items.into_iter().map(|item| self.item(item)).collect())
            })
            .collect();
        let handled = self.handled.as_ref().map(watch::Sender::subscribe);
        let stream = futures::stream::unfold(
            (pieces.into_iter(), handled),
            |(mut pieces, mut handled)| async move {
                let (hold_until, items) = pieces.next()?;
                if let (Some(line), Some(handled)) = (hold_until, handled.as_mut()) {
                    // The provider is gone when this fails, so the answer ends there.
                    handled.wait_for(|reached| *reached >= line).await.ok()?;
                }
                Some((futures::stream::iter(items), (pieces, handled)))
            },
        )
        .flatten();
        Ok(Box::pin(stream))
    }
}

impl fmt::Debug for ReplayProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("ReplayProvider")
            .field("id", &self.id)
            .field("served", &state.served)
            .field("remaining", &state.exchanges.len())
            .field("failed", &state.failure.is_some())
            .field("paced", &self.handled.is_some())
            .finish()
    }
}

impl Failure {
    fn to_error(&self) -> TestSupportError {
        match self.clone() {
            Failure::Exhausted { request } => TestSupportError::UnexpectedRequest { request },
            Failure::Mismatch { line, pointer, expected, actual } => {
                TestSupportError::RequestMismatch { line, pointer, expected, actual }
            }
        }
    }

    fn to_provider_error(&self) -> ProviderError {
        let code = match self {
            Failure::Exhausted { .. } => ReplayProvider::EXHAUSTED_CODE,
            Failure::Mismatch { .. } => ReplayProvider::MISMATCH_CODE,
        };
        ProviderError::Api {
            status: None,
            code: Some(code.to_owned()),
            message: self.to_error().to_string(),
        }
    }
}

impl ErrorRecord {
    fn into_error(self) -> ProviderError {
        match self {
            ErrorRecord::Unauthorized {} => ProviderError::Unauthorized,
            ErrorRecord::RateLimited { retry_after_ms } => ProviderError::RateLimited {
                retry_after: retry_after_ms.map(Duration::from_millis),
            },
            ErrorRecord::NotLoggedIn {} => ProviderError::NotLoggedIn,
            ErrorRecord::Incomplete {} => ProviderError::Incomplete,
            // NOTE: through the classifier that the real providers use, so a transcript
            // can refuse a request as larger than the model's window.
            ErrorRecord::Api { status, code, message } => ProviderError::api(status, code, message),
        }
    }
}

fn replay_id() -> ProviderId {
    match ProviderId::new(DEFAULT_ID) {
        Ok(id) => id,
        Err(_) => unreachable!("`replay` is lowercase ASCII letters, a valid provider id"),
    }
}

/// The exchanges of `transcript`, validated.
fn exchanges(transcript: &Transcript) -> Result<VecDeque<Exchange>, TestSupportError> {
    let mut exchanges: VecDeque<Exchange> = VecDeque::new();
    let mut since_provider: Option<usize> = None;
    for entry in transcript.entries() {
        let line = entry.line;
        match &entry.record {
            Record::ExpectOutbound(Outbound::ProviderRequest(body)) => {
                serde_json::from_value::<Request>(body.clone())
                    .map_err(|source| TestSupportError::ProviderRecord { line, source })?;
                exchanges.push_back(Exchange { line, expected: body.clone(), pieces: Vec::new() });
                since_provider = None;
            }
            Record::EmitInbound(Inbound::ProviderSse(body)) => {
                let Some(exchange) = exchanges.back_mut() else {
                    return Err(TestSupportError::InvalidRecord {
                        line,
                        problem: "a provider_sse record must follow a provider_request record",
                    });
                };
                let ended = exchange
                    .pieces
                    .iter()
                    .flat_map(|piece| &piece.items)
                    .any(|item| matches!(item, Item::Error(_)));
                let items = items(body, line)?;
                if ended && !items.is_empty() {
                    return Err(TestSupportError::InvalidRecord {
                        line,
                        problem: "an answer has events after its error event",
                    });
                }
                exchange.pieces.push(Piece { hold_until: since_provider.take(), items });
            }
            _ => since_provider = Some(line),
        }
    }
    Ok(exchanges)
}

/// The answer items of one `provider_sse` body.
fn items(body: &str, line: usize) -> Result<Vec<Item>, TestSupportError> {
    let events =
        sse::parse(body).map_err(|problem| TestSupportError::InvalidRecord { line, problem })?;
    let mut items = Vec::with_capacity(events.len());
    for event in events {
        if items.iter().any(|item| matches!(item, Item::Error(_))) {
            return Err(TestSupportError::InvalidRecord {
                line,
                problem: "an answer has events after its error event",
            });
        }
        let data: Value = serde_json::from_str(&event.data)
            .map_err(|source| TestSupportError::ProviderRecord { line, source })?;
        let item = match event.event.as_str() {
            "message" => {
                serde_json::from_value::<ProviderEvent>(data.clone())
                    .map_err(|source| TestSupportError::ProviderRecord { line, source })?;
                Item::Event(data)
            }
            "error" => {
                serde_json::from_value::<ErrorRecord>(data.clone())
                    .map_err(|source| TestSupportError::ProviderRecord { line, source })?;
                Item::Error(data)
            }
            _ => {
                return Err(TestSupportError::InvalidRecord {
                    line,
                    problem: "a provider_sse event must have the type message or error",
                });
            }
        };
        items.push(item);
    }
    Ok(items)
}

/// The JSON pointer of the first place where `expected` and `actual` differ, member
/// names in sorted order: an empty string when the documents differ at the top.
fn first_difference(expected: &Value, actual: &Value) -> String {
    let mut pointer = String::new();
    let (mut expected, mut actual) = (expected, actual);
    loop {
        let (next_expected, next_actual) = match (expected, actual) {
            (Value::Object(left), Value::Object(right)) => {
                let names: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
                let Some(name) = names.into_iter().find(|name| left.get(*name) != right.get(*name))
                else {
                    return pointer;
                };
                pointer.push('/');
                pointer.push_str(&name.replace('~', "~0").replace('/', "~1"));
                (left.get(name), right.get(name))
            }
            (Value::Array(left), Value::Array(right)) => {
                let Some(index) = (0..left.len().max(right.len()))
                    .find(|&index| left.get(index) != right.get(index))
                else {
                    return pointer;
                };
                pointer.push('/');
                pointer.push_str(&index.to_string());
                (left.get(index), right.get(index))
            }
            _ => return pointer,
        };
        match (next_expected, next_actual) {
            (Some(left), Some(right)) => (expected, actual) = (left, right),
            _ => return pointer,
        }
    }
}

#[cfg(test)]
mod tests;
