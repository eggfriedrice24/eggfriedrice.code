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
//! for [`PROBE_AFTER`] sends a ping and its request together, with no wait for the
//! pong, so a connection that works costs no round trip. A connection that died without
//! a close gives neither the pong nor an event, so it costs at most [`PROBE_TIMEOUT`]
//! before the call goes over HTTP, and never the read timeout. After [`PING_FIRST`]
//! without a call, as after a sleep of the laptop, the request waits for the pong.
//!
//! A warm-up ([`Sockets::warm`]) makes the connection of the next call ready before
//! the call: it opens the conversation's connection, which then waits free, or pings a
//! free one that had no call for [`PROBE_AFTER`]. A call that comes during the
//! handshake of a warm-up waits for it, so the two open one connection. It sends no
//! request, so no model runs.
//!
//! A call goes over HTTP when the server did not act on its request: when the
//! connection cannot be opened, when the server refuses the upgrade, when a quiet
//! connection breaks, ends or stays silent for [`PROBE_TIMEOUT`] before its pong and
//! before any event, or when the server does not take the request
//! (it closes the connection, or sends an `error` event without an error status that
//! answers the request, before the first event of an answer). The HTTP request is then
//! the first and only model call. After a failure that says that WebSockets do not work
//! now, every call goes over HTTP for [`PAUSE`]. An `error` event with an HTTP error
//! status, such as a rate limit or a context overflow, is the server's answer to the
//! request, and the call fails with it as with that answer over HTTP. Once the request
//! has gone out, a failure that does not show that the server refused it (a broken
//! connection, a connection that ends without a close, or a timeout) is the call's
//! failure, exactly as on the HTTP path, because the server may have acted on it and
//! sending the call again could run the model twice. The one exception is the quiet
//! connection above before its pong and its first event: a server that read the
//! request read the ping before it and answered it at once, so no pong and no event
//! in [`PROBE_TIMEOUT`] most likely means a dead connection. The rest risk: a
//! connection that is only late, such as on a path that stalls for more than
//! [`PROBE_TIMEOUT`], still holds the request, and the server can read it after HTTP
//! sent the call, so the model can run two times. A request that went out cannot be
//! taken back. A call after [`PING_FIRST`] has no such risk: its request goes out only
//! after the pong.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_http::WebSocket;
use efr_provider::{CallNotice, CallNotices, ProviderError, ProviderEvent, ProviderStream};
use efr_stdx::time::Clock;
use futures::stream;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tracing::Instrument as _;

use crate::openai::convert::ResponsesBody;
use crate::openai::sse_events::EventMapper;
use crate::openai::timing::{Route, Timing};

mod connection;
mod continuation;

pub(crate) use connection::Rejection;
use connection::{Connection, Create, Delivery, Limits, Refusal};
use continuation::{Outgoing, Plan, plan};

/// How long an unused connection stays open. A turn's calls follow each other within
/// seconds; the next prompt may come minutes later, and a connection that waits for
/// it saves its setup.
pub(crate) const IDLE: Duration = Duration::from_secs(10 * 60);

/// A connection takes no new call after this age, before the server's limit of 60
/// minutes for one connection (`websocket_connection_limit_reached` in Codex).
const MAX_AGE: Duration = Duration::from_secs(55 * 60);

/// A call on a connection that had no call for this long sends a ping with its
/// request. A turn's calls follow each other within seconds, and those skip the ping;
/// the next prompt may come minutes later, after a sleep of the laptop or a change of
/// network that dropped the connection without a close.
pub(crate) const PROBE_AFTER: Duration = Duration::from_secs(30);

/// How long that ping may go without its pong and without an event before the
/// connection counts as dead and the call goes over HTTP.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A call on a connection that had no call for this long waits for the pong of its
/// ping before its request goes out, as after a sleep of the laptop. Below this, the
/// request goes out with the ping and costs no round trip.
pub(crate) const PING_FIRST: Duration = Duration::from_secs(5 * 60);

/// A warm-up opens a new connection in place of one that has less than this left of
/// [`MAX_AGE`], so the prompt that follows the warm-up finds a connection that still
/// takes it.
const WARM_AGE_MARGIN: Duration = Duration::from_secs(5 * 60);

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
    /// The conversations whose connection is being opened, by a call or by a warm-up.
    /// A call waits for the opening of a warm-up, so a warm-up and the call that follows
    /// it open one connection; a warm-up opens none while any opening runs.
    opening: HashMap<String, Noted>,
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
            ping_first: PING_FIRST,
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
    /// `notices` hears of a wait for the handshake of a warm-up as of a connect;
    /// `connect` tells its own.
    pub(crate) async fn stream<F, Fut>(
        &self,
        key: &str,
        body: &ResponsesBody,
        connect: F,
        notices: &dyn CallNotices,
        timing: Timing,
        span: tracing::Span,
    ) -> Attempt
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<WebSocket, ConnectError>>,
    {
        efr_stdx::mark::hit("ws.begin");
        let waited = self.wait_opening(key, notices).await;
        let mut taken = match self.take(key) {
            Ok(Some(taken)) => {
                if waited {
                    notices.notice(CallNotice::Connected);
                }
                taken
            }
            Ok(None) => match self.open(key, connect).await {
                Ok(taken) => taken,
                Err(reason) => {
                    efr_stdx::mark::hit("ws.open_failed");
                    return Attempt::Http(reason);
                }
            },
            Err(reason) => {
                efr_stdx::mark::hit("ws.skipped");
                return Attempt::Skipped(reason);
            }
        };
        efr_stdx::mark::hit_for("ws.taken", if taken.new { "new" } else { "reused" });
        let outgoing = Outgoing::of(body);
        let plan = plan(taken.last.as_ref(), &outgoing);
        let route = Route {
            transport: "websocket",
            connection: if taken.new { "new" } else { "reused" },
            input: plan.name(),
        };
        let (previous, input) = match plan {
            Plan::Full => (None, body.input()),
            Plan::Incremental { previous, input } => (Some(previous), input),
        };
        let message =
            match serde_json::to_string(&outgoing.settings.websocket_create(previous, input)) {
                Ok(message) => message,
                Err(error) => {
                    return Attempt::Http(format!("the request could not be encoded: {error}"));
                }
            };
        efr_stdx::mark::hit_for("ws.encoded", route.input);
        let (events, mut receiver) = mpsc::channel(EVENTS);
        let Outgoing { settings, prints, .. } = outgoing;
        let create = Create { message, settings, prints, events };
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
                        efr_stdx::mark::hit("ws.accepted");
                        timing.accepted();
                        self.state().unauthorized = false;
                        break;
                    }
                }
                Some(Delivery::Refused(Refusal { reason, health })) => {
                    efr_stdx::mark::hit("ws.refused");
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
        let connection = Arc::clone(&taken.connection);
        Attempt::Answered(answer(early, receiver, connection, timing, span))
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
        let _opening = self.opening(key, false);
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
        efr_stdx::mark::hit("ws.connected");
        let opened_at = self.clock.now();
        let connection = Arc::new(Connection::start(
            socket,
            Arc::clone(&self.clock),
            self.limits,
            opened_at,
            true,
        ));
        let mut state = self.state();
        // NOTE: two calls of one conversation can both find no connection and both
        // open one. The first stays the conversation's connection; the second serves
        // only its own call, and its answer keeps it open until the answer ends.
        if !state.connections.get(key).is_some_and(|existing| !existing.is_closed()) {
            state.connections.insert(key.to_owned(), Arc::clone(&connection));
        }
        drop(state);
        Ok(Taken::new(connection, None, true))
    }

    /// Makes the connection of the conversation `key` ready for its next call, for a
    /// warm-up: a free connection that had no call for [`PROBE_AFTER`] gets a ping, so
    /// its pong lets the call skip the ping, or its silence closes it; without a
    /// connection, or with one near [`MAX_AGE`], it opens one with `connect`, which
    /// then waits free for the call. It sends no request. Nothing happens while
    /// WebSockets are paused, while a call runs on the connection or while another
    /// opening for `key` runs. A failure only shows in the log, and only a handshake
    /// that times out pauses WebSockets: a warm-up can run before the network is up,
    /// as after a sleep of the laptop, so the call judges its own connect. Returns true
    /// when a connection is ready or on its way.
    pub(crate) async fn warm<F, Fut>(&self, key: &str, connect: F) -> bool
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<WebSocket, ConnectError>>,
    {
        let now = self.clock.now();
        let opening = {
            let mut state = self.state();
            if state.paused_until.is_some_and(|until| now < until) {
                return false;
            }
            state.connections.retain(|_, connection| !connection.is_finished());
            if state.opening.contains_key(key) {
                return true;
            }
            let ready = state.connections.get(key).filter(|connection| {
                !connection.is_closed()
                    && connection.is_younger(now, MAX_AGE.saturating_sub(WARM_AGE_MARGIN))
            });
            if let Some(connection) = ready {
                if !connection.is_busy() {
                    efr_stdx::mark::hit("ws.warm.probe");
                    connection.probe();
                }
                return true;
            }
            Opening::start(&self.state, &mut state, key, true)
        };
        efr_stdx::mark::hit("ws.warm.open");
        let socket = match self.clock.timeout(CONNECT_TIMEOUT, connect()).await {
            Ok(Ok(socket)) => socket,
            Ok(Err(ConnectError { reason, .. })) => {
                // NOTE: no pause: a network that is not up yet at the first key of a
                // line would send each call of the next minutes over HTTP.
                tracing::debug!(reason = %reason, "the warm-up could not open the websocket");
                return false;
            }
            Err(_) => {
                tracing::debug!("the handshake of the warm-up timed out");
                self.pause();
                return false;
            }
        };
        efr_stdx::mark::hit("ws.warm.connected");
        let connection = Arc::new(Connection::start(
            socket,
            Arc::clone(&self.clock),
            self.limits,
            self.clock.now(),
            false,
        ));
        let mut state = self.state();
        // NOTE: a call that found no connection and did not wait opens its own; the
        // first one stays the conversation's connection.
        let kept = state.connections.get(key).is_some_and(|existing| {
            !existing.is_closed()
                && existing.is_younger(now, MAX_AGE.saturating_sub(WARM_AGE_MARGIN))
        });
        if !kept {
            state.connections.insert(key.to_owned(), connection);
        }
        drop(state);
        drop(opening);
        true
    }

    /// Notes that a call (`warm` false) or a warm-up opens the connection of `key`,
    /// until the guard drops.
    fn opening(&self, key: &str, warm: bool) -> Opening<'_> {
        let mut state = self.state();
        Opening::start(&self.state, &mut state, key, warm)
    }

    /// Waits for the opening of the connection of `key` by a warm-up that runs now, at
    /// most [`CONNECT_TIMEOUT`], and tells `notices` that the call connects. A call does
    /// not wait for another call's opening: that connection serves only the other
    /// call. True when it waited.
    async fn wait_opening(&self, key: &str, notices: &dyn CallNotices) -> bool {
        let waiting = self
            .state()
            .opening
            .get(key)
            .filter(|noted| noted.warm)
            .map(|noted| noted.done.clone());
        let Some(mut done) = waiting else {
            return false;
        };
        efr_stdx::mark::hit("ws.wait_opening");
        // NOTE: the warm-up connects with no notices, so the call tells its own wait.
        notices.notice(CallNotice::Connecting);
        let _ = self.clock.timeout(CONNECT_TIMEOUT, done.wait_for(|done| *done)).await;
        true
    }

    /// True while `key` has a connection that takes calls.
    #[cfg(test)]
    pub(crate) fn has(&self, key: &str) -> bool {
        self.state().connections.get(key).is_some_and(|connection| !connection.is_closed())
    }

    /// True while a call or a warm-up opens the connection of `key`.
    #[cfg(test)]
    pub(crate) fn is_opening(&self, key: &str) -> bool {
        self.state().opening.contains_key(key)
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

/// An opening noted in [`State::opening`].
#[derive(Debug)]
struct Noted {
    /// Turns true when the opening has ended, also when it failed.
    done: watch::Receiver<bool>,
    /// A warm-up opens the connection, so a call may wait for it.
    warm: bool,
}

/// An opening of the connection of one conversation, noted in [`State::opening`] until
/// it drops.
struct Opening<'a> {
    state: &'a Mutex<State>,
    key: String,
    done: watch::Sender<bool>,
}

impl<'a> Opening<'a> {
    /// Notes the opening of `key` by a warm-up (`warm`) or a call in `locked`, the guard
    /// of `state`. A note that is there already is replaced; its waiters still see its
    /// own end.
    fn start(state: &'a Mutex<State>, locked: &mut State, key: &str, warm: bool) -> Opening<'a> {
        let (done, waiting) = watch::channel(false);
        locked.opening.insert(key.to_owned(), Noted { done: waiting, warm });
        Opening { state, key: key.to_owned(), done }
    }
}

impl Drop for Opening<'_> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let ours = state
            .opening
            .get(&self.key)
            .is_some_and(|noted| noted.done.same_channel(&self.done.subscribe()));
        if ours {
            state.opening.remove(&self.key);
        }
        drop(state);
        self.done.send_replace(true);
    }
}

/// The canonical events of an answer: the events that arrived before the server took
/// the call, then the rest as they come.
fn answer(
    early: Vec<Value>,
    receiver: mpsc::Receiver<Delivery>,
    connection: Arc<Connection>,
    timing: Timing,
    span: tracing::Span,
) -> ProviderStream {
    let state = Answer {
        receiver: Some(receiver),
        _connection: connection,
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
    /// The connection that serves the answer. Dropping the last handle stops its task,
    /// so the answer holds one: the map of connections may hold another connection of
    /// the conversation, or none.
    _connection: Arc<Connection>,
    early: std::collections::VecDeque<Value>,
    mapper: EventMapper,
    pending: std::collections::VecDeque<Result<ProviderEvent, ProviderError>>,
    timing: Timing,
}

impl Answer {
    async fn next(&mut self) -> Option<Result<ProviderEvent, ProviderError>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                match &item {
                    Ok(_) => self.timing.first_event(),
                    // NOTE: an error ends the answer, so this line comes once per call. A
                    // call on the websocket has no request id of its own.
                    Err(error) => tracing::info!(error = %error, "the model call failed"),
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
