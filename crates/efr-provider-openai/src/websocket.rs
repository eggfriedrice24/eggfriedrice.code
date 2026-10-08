//! The Responses WebSocket transport.
//!
//! A model call can go out as a `response.create` message on a WebSocket to
//! `<base_url>/responses` instead of a `POST`. The server answers with the same events
//! as the HTTP stream, one JSON event per text message, so the same [`EventMapper`]
//! turns them into canonical events. The transport saves the TCP and TLS setup of each
//! call, and on a connection that holds the previous answer a call sends only its new
//! input items (see [`continuation`]).
//!
//! [`Sockets`] keeps one connection for each conversation, by the request's
//! `prompt_cache_key`, which the conversation sets to its id. A connection serves one
//! call at a time; it stays open between the calls of a turn and between turns, and
//! closes after [`IDLE`] without a call, when the server closes it, after any failure,
//! and before the server's limit of 60 minutes. A call on a connection that had no call
//! for [`PROBE_AFTER`] first pings the server, so a connection that died without a close
//! costs at most [`PROBE_TIMEOUT`] before the call goes over HTTP, and never the read
//! timeout.
//!
//! A call that the socket cannot serve goes over HTTP: when the connection cannot be
//! opened, when the server refuses the upgrade, or when the server does not take the
//! request (it sends an `error` event or closes the connection before the first event
//! of an answer). The server has then not started an answer, so the HTTP request is the
//! first and only model call. After a failure that says that WebSockets do not work
//! now, every call goes over HTTP for [`PAUSE`]. An `error` event with an HTTP error
//! status, such as a rate limit or a context overflow, is the server's answer to the
//! request, and the call fails with it as with that answer over HTTP. Once the request has gone out, a
//! failure that does not show that the server refused it (a broken connection, a
//! connection that ends without a close, or a timeout) is the call's failure, exactly
//! as on the HTTP path, because the server may have acted on it and sending the call
//! again could run the model twice.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_http::WebSocket;
use efr_provider::{ProviderError, ProviderEvent, ProviderStream};
use efr_stdx::time::Clock;
use futures::stream;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::Instrument as _;

use crate::convert::ResponsesBody;
use crate::sse_events::EventMapper;
use crate::timing::{Route, Timing};

mod connection;
mod continuation;

pub(crate) use connection::Rejection;
use connection::{Connection, Create, Delivery, Limits, Refusal};
use continuation::{Plan, plan};

/// How long an unused connection stays open. A turn's calls follow each other within
/// seconds; the next prompt may come minutes later, and a connection that waits for
/// it saves its setup.
pub(crate) const IDLE: Duration = Duration::from_secs(10 * 60);

/// A connection takes no new call after this age, before the server's limit of 60
/// minutes for one connection (`websocket_connection_limit_reached` in Codex).
const MAX_AGE: Duration = Duration::from_secs(55 * 60);

/// A call on a connection that had no call for this long pings the server first. A
/// turn's calls follow each other within seconds, and those skip the ping; the next
/// prompt may come minutes later, after a sleep of the laptop or a change of network
/// that dropped the connection without a close.
pub(crate) const PROBE_AFTER: Duration = Duration::from_secs(30);

/// How long the pong of that ping may take before the connection counts as dead.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long every call goes over HTTP after a failure that says that WebSockets do not
/// work now, such as a refused upgrade.
pub(crate) const PAUSE: Duration = Duration::from_secs(5 * 60);

/// The longest wait for the handshake, as Codex's `websocket_connect_timeout`.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The longest silence inside an answer: the read timeout of the HTTP client, and
/// Codex's `stream_idle_timeout`.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a `response.create` or a close may take to go out.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an interrupted answer may take to end before its connection closes.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// How many events wait for the caller.
const EVENTS: usize = 64;

/// The outcome of a try on the WebSocket.
pub(crate) enum Attempt {
    /// The server took the call; the stream carries its answer.
    Answered(ProviderStream),
    /// The socket could not serve the call, for `reason`; it goes over HTTP.
    Http(String),
    /// The server answered the call with an error status; the call fails with it.
    Rejected(Rejection),
    /// The socket was not tried, for `reason`: WebSockets are paused, or the
    /// conversation's connection is busy. The call goes over HTTP.
    Skipped(String),
}

/// Why a connection could not be opened.
#[derive(Debug)]
pub(crate) struct ConnectError {
    /// What failed, for the log.
    pub(crate) reason: String,
    /// What the failure says about WebSockets.
    pub(crate) health: Health,
}

/// What a failure before the server took a call says about WebSockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Health {
    /// Nothing: a stale connection or a routine error. The next call tries again.
    Fine,
    /// WebSockets do not work now, such as after a refused upgrade: every call goes
    /// over HTTP for [`PAUSE`].
    Broken,
    /// The server refused the token. The HTTP path refreshes it, so one refusal does
    /// not pause WebSockets; a second one in a row does, because the socket then
    /// refuses a token that HTTP takes.
    Unauthorized,
}

/// The open connections of one provider, one for each conversation.
#[derive(Debug)]
pub(crate) struct Sockets {
    clock: Arc<dyn Clock>,
    limits: Limits,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    connections: HashMap<String, Arc<Connection>>,
    paused_until: Option<Timestamp>,
    /// The last call that the socket did not serve failed on a refused token.
    unauthorized: bool,
}

/// A connection taken for one call. Dropped before its request went out, it frees the
/// connection again.
struct Taken {
    connection: Arc<Connection>,
    last: Option<continuation::Continuation>,
    new: bool,
    sent: bool,
}

impl Taken {
    fn new(
        connection: Arc<Connection>,
        last: Option<continuation::Continuation>,
        new: bool,
    ) -> Taken {
        Taken { connection, last, new, sent: false }
    }

    /// Hands `create` to the connection's task. False when the task has ended.
    async fn send(&mut self, create: Create) -> bool {
        self.sent = self.connection.send(create).await;
        self.sent
    }
}

impl Drop for Taken {
    fn drop(&mut self) {
        if !self.sent {
            self.connection.release();
        }
    }
}

impl Sockets {
    /// No connections yet; `clock` times the idle close, the pause and every wait.
    pub(crate) fn new(clock: Arc<dyn Clock>) -> Sockets {
        let limits = Limits {
            idle: IDLE,
            probe_after: PROBE_AFTER,
            probe: PROBE_TIMEOUT,
            read: READ_TIMEOUT,
            send: SEND_TIMEOUT,
            drain: DRAIN_TIMEOUT,
        };
        Sockets { clock, limits, state: Mutex::new(State::default()) }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends `body` on the connection of the conversation `key`, opening it with
    /// `connect` when there is none. `timing` measures the call; `span` is its span.
    pub(crate) async fn stream<F, Fut>(
        &self,
        key: &str,
        body: &ResponsesBody,
        connect: F,
        timing: Timing,
        span: tracing::Span,
    ) -> Attempt
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<WebSocket, ConnectError>>,
    {
        let mut taken = match self.take(key) {
            Ok(Some(taken)) => taken,
            Ok(None) => match self.open(key, connect).await {
                Ok(taken) => taken,
                Err(reason) => return Attempt::Http(reason),
            },
            Err(reason) => return Attempt::Skipped(reason),
        };
        let plan = plan(taken.last.as_ref(), body);
        let route = Route {
            transport: "websocket",
            connection: if taken.new { "new" } else { "reused" },
            input: plan.name(),
        };
        let (previous, input) = match plan {
            Plan::Full => (None, body.input()),
            Plan::Incremental { previous, input } => (Some(previous), input),
        };
        let message = match serde_json::to_string(&body.websocket_create(previous, input)) {
            Ok(message) => message,
            Err(error) => {
                return Attempt::Http(format!("the request could not be encoded: {error}"));
            }
        };
        let (events, mut receiver) = mpsc::channel(EVENTS);
        let create =
            Create { message, settings: body.settings(), input: body.input().to_vec(), events };
        if !taken.send(create).await {
            return Attempt::Http("the websocket closed before the request".to_owned());
        }
        let mut timing = timing.on(route);
        let mut early = Vec::new();
        loop {
            match receiver.recv().await {
                Some(Delivery::Event(event)) => {
                    let answer = event
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with("response."));
                    early.push(event);
                    if answer {
                        timing.accepted();
                        self.state().unauthorized = false;
                        break;
                    }
                }
                Some(Delivery::Refused(Refusal { reason, health })) => {
                    self.judge(health);
                    return Attempt::Http(reason);
                }
                Some(Delivery::Rejected(rejection)) => {
                    self.state().unauthorized = false;
                    return Attempt::Rejected(rejection);
                }
                Some(Delivery::Failed(error)) => {
                    // NOTE: the server took the request, or it may have: the connection
                    // broke or went silent after the request went out. The call fails
                    // as on the HTTP path and does not go out again.
                    return Attempt::Answered(Box::pin(stream::iter([Err(error)])));
                }
                None => return Attempt::Http("the websocket task ended".to_owned()),
            }
        }
        Attempt::Answered(answer(early, receiver, timing, span))
    }

    /// The free connection of `key`, taken for one call. `Ok(None)` when there is none
    /// to take, and an error when the call must go over HTTP.
    fn take(&self, key: &str) -> Result<Option<Taken>, String> {
        let now = self.clock.now();
        let mut state = self.state();
        if let Some(until) = state.paused_until {
            if now < until {
                return Err("websockets are paused after a failure".to_owned());
            }
            state.paused_until = None;
        }
        state.connections.retain(|_, connection| !connection.is_finished());
        let Some(connection) = state.connections.get(key) else {
            return Ok(None);
        };
        if connection.is_closed() {
            return Ok(None);
        }
        if connection.is_busy() {
            return Err("the conversation's websocket is busy with another call".to_owned());
        }
        match connection.take(now, MAX_AGE) {
            Some(last) => Ok(Some(Taken::new(Arc::clone(connection), last, false))),
            None => {
                state.connections.remove(key);
                Ok(None)
            }
        }
    }

    /// Opens the connection of `key` with `connect` and takes it for one call.
    async fn open<F, Fut>(&self, key: &str, connect: F) -> Result<Taken, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<WebSocket, ConnectError>>,
    {
        let watch = efr_stdx::time::Stopwatch::start();
        let socket = match self.clock.timeout(CONNECT_TIMEOUT, connect()).await {
            Ok(Ok(socket)) => socket,
            Ok(Err(ConnectError { reason, health })) => {
                self.judge(health);
                return Err(reason);
            }
            Err(_) => {
                self.pause();
                return Err("the websocket handshake timed out".to_owned());
            }
        };
        tracing::debug!(phase = "provider_connect", transport = "websocket", elapsed_ms = %watch, "phase=provider_connect transport=websocket elapsed_ms={}", watch);
        let opened_at = self.clock.now();
        let connection =
            Arc::new(Connection::start(socket, Arc::clone(&self.clock), self.limits, opened_at));
        self.state().connections.insert(key.to_owned(), Arc::clone(&connection));
        Ok(Taken::new(connection, None, true))
    }

    /// True while the connection of `key` runs or drains a call.
    #[cfg(test)]
    pub(crate) fn is_busy(&self, key: &str) -> bool {
        self.state().connections.get(key).is_some_and(|connection| connection.is_busy())
    }

    /// Pauses WebSockets when `health` says so.
    fn judge(&self, health: Health) {
        let pause = match health {
            Health::Fine => false,
            Health::Broken => true,
            Health::Unauthorized => {
                // NOTE: the flag stays set until a call goes through, so after the
                // pause one more refusal pauses again.
                let mut state = self.state();
                std::mem::replace(&mut state.unauthorized, true)
            }
        };
        if pause {
            self.pause();
        }
    }

    /// Sends every call over HTTP for [`PAUSE`].
    fn pause(&self) {
        let until =
            self.clock.now().checked_add(SignedDuration::try_from(PAUSE).unwrap_or_default());
        if let Ok(until) = until {
            tracing::debug!(pause = ?PAUSE, "websockets are paused");
            self.state().paused_until = Some(until);
        }
    }
}

/// The canonical events of an answer: the events that arrived before the server took
/// the call, then the rest as they come.
fn answer(
    early: Vec<Value>,
    receiver: mpsc::Receiver<Delivery>,
    timing: Timing,
    span: tracing::Span,
) -> ProviderStream {
    let state = Answer {
        receiver: Some(receiver),
        early: early.into(),
        mapper: EventMapper::new(),
        pending: std::collections::VecDeque::new(),
        timing,
    };
    Box::pin(stream::unfold(state, move |mut state| {
        let span = span.clone();
        async move { state.next().await.map(|item| (item, state)) }.instrument(span)
    }))
}

struct Answer {
    /// The events from the task, dropped once the answer has ended. Dropping it early,
    /// with the stream, makes the task interrupt the answer.
    receiver: Option<mpsc::Receiver<Delivery>>,
    early: std::collections::VecDeque<Value>,
    mapper: EventMapper,
    pending: std::collections::VecDeque<Result<ProviderEvent, ProviderError>>,
    timing: Timing,
}

impl Answer {
    async fn next(&mut self) -> Option<Result<ProviderEvent, ProviderError>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                if item.is_ok() {
                    self.timing.first_event();
                }
                return Some(item);
            }
            let receiver = self.receiver.as_mut()?;
            let delivery = match self.early.pop_front() {
                Some(event) => Some(Delivery::Event(event)),
                None => receiver.recv().await,
            };
            let ended = match delivery {
                Some(Delivery::Event(event)) => match self.mapper.map_message(&event) {
                    Ok(events) => {
                        self.pending.extend(events.into_iter().map(Ok));
                        self.mapper.is_done()
                    }
                    Err(error) => {
                        self.pending.push_back(Err(error));
                        true
                    }
                },
                Some(Delivery::Failed(error)) => {
                    self.pending.push_back(Err(error));
                    true
                }
                Some(Delivery::Refused(Refusal { reason, .. })) => {
                    // NOTE: a refusal comes only before the server takes a call, and
                    // this stream starts after that; it is a broken promise of the task.
                    tracing::debug!(reason = %reason, "a refusal after the answer started");
                    self.pending.push_back(Err(ProviderError::Incomplete));
                    true
                }
                Some(Delivery::Rejected(Rejection { status, .. })) => {
                    // NOTE: the same broken promise as a refusal.
                    tracing::debug!(status, "a rejection after the answer started");
                    self.pending.push_back(Err(ProviderError::Incomplete));
                    true
                }
                None => {
                    self.pending.push_back(Err(ProviderError::Incomplete));
                    true
                }
            };
            if ended {
                self.receiver = None;
            }
        }
    }
}

#[cfg(test)]
mod tests;
