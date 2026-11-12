//! One Responses WebSocket connection and the task that owns it.
//!
//! The task sends one `response.create` at a time and passes the server's events to the
//! caller of that request. It keeps reading after the caller has gone: it then sends
//! `response.interrupt` for the answer and reads until the answer ends, so the
//! connection is clean for the next request. While no request runs, it closes the
//! connection after [`Limits::idle`] or when the server closes it. A request that comes
//! after [`Limits::probe_after`] without one goes out right after a ping, with no wait
//! for the pong. A connection that died without a close, such as after a change of
//! network, gives neither the pong nor a message, so it fails after [`Limits::probe`]
//! and the call goes over HTTP. After [`Limits::ping_first`] without a request, such as
//! after a sleep of the laptop, the request waits for the pong before it goes out, so a
//! connection that is only late never gets a request that HTTP also sent.
//!
//! A warm-up asks the free connection for a check ([`Connection::probe`]): after
//! [`Limits::probe_after`] without a request, the task sends a ping. Its pong makes the
//! next request skip the ping and starts the idle time again; no pong within
//! [`Limits::probe`] closes the connection. A request that comes before the pong takes
//! that ping over instead of sending its own. A connection that a warm-up opened starts
//! free and idle.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_http::{HttpError, Pong, WebSocket, WsMessage};
use efr_provider::ProviderError;
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tracing::Instrument as _;

use super::Health;
use super::continuation::{Continuation, Print};
use crate::openai::convert::ResponsesBody;

/// The error codes of an `error` event that say nothing about the socket's health: the
/// server's limit of 60 minutes for one connection, and an answer that this connection
/// no longer holds (`codex-rs/codex-api/src/endpoint/responses_websocket.rs`, which
/// retries both).
const ROUTINE_ERRORS: &[&str] =
    &["websocket_connection_limit_reached", "previous_response_not_found"];

/// The event types that end an answer.
const ENDS: &[&str] = &["response.completed", "response.incomplete", "response.failed", "error"];

/// The times a connection works with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// How long an unused connection stays open.
    pub(crate) idle: Duration,
    /// How long a connection may go without a request before the next request sends a
    /// ping with it.
    pub(crate) probe_after: Duration,
    /// How long that ping may go without its pong and without a message of the
    /// server.
    pub(crate) probe: Duration,
    /// How long a connection may go without a request before the next request waits
    /// for the pong of its ping before it goes out.
    pub(crate) ping_first: Duration,
    /// The longest silence inside an answer, as the read timeout of the HTTP path.
    pub(crate) read: Duration,
    /// How long a `response.create` may take to go out.
    pub(crate) send: Duration,
    /// How long an interrupted answer may take to end.
    pub(crate) drain: Duration,
}

/// What the task passes to the caller of a request.
#[derive(Debug)]
pub(crate) enum Delivery {
    /// A server event, as JSON.
    Event(Value),
    /// The request failed after the server took it.
    Failed(ProviderError),
    /// The request failed before the server took it, so it may go over HTTP.
    Refused(Refusal),
    /// The server answered the request with an error status before an answer, as an
    /// HTTP error answer: the request itself failed, and over HTTP it would fail the
    /// same way.
    Rejected(Rejection),
}

/// An `error` event with an HTTP error status, the server's answer to a request.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rejection {
    /// The status, from 400 to 499.
    pub(crate) status: u16,
    /// The whole event, with its `error` object and maybe its `headers`.
    pub(crate) event: Value,
}

/// Why the server did not take a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// What happened, for the log.
    pub(crate) reason: String,
    /// What the failure says about WebSockets.
    pub(crate) health: Health,
}

/// One request for the task.
#[derive(Debug)]
pub(crate) struct Create {
    /// The `response.create` message.
    pub(crate) message: String,
    /// Every field of the request but `input`.
    pub(crate) settings: ResponsesBody,
    /// The prints of the request's whole input; `None` keeps no continuation.
    pub(crate) prints: Option<Vec<Print>>,
    /// Where the events go.
    pub(crate) events: mpsc::Sender<Delivery>,
}

/// What the pool and the task share.
#[derive(Debug, Default)]
struct Shared {
    /// A request is running or draining.
    busy: bool,
    /// The task has ended; the connection is gone.
    closed: bool,
    /// The last completed answer, for the next request.
    last: Option<Continuation>,
}

/// The handle of one connection.
#[derive(Debug)]
pub(crate) struct Connection {
    commands: mpsc::Sender<Create>,
    shared: Arc<Mutex<Shared>>,
    opened_at: Timestamp,
    /// Asks the task to check a quiet connection with a ping, for a warm-up.
    probes: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Connection {
    /// Starts the task on `socket`, opened at `opened_at`. With `busy` the caller that
    /// opened it sends the first request at once; else, for a warm-up, it starts free
    /// and idle.
    pub(crate) fn start(
        socket: WebSocket,
        clock: Arc<dyn Clock>,
        limits: Limits,
        opened_at: Timestamp,
        busy: bool,
    ) -> Connection {
        let shared = Arc::new(Mutex::new(Shared { busy, ..Shared::default() }));
        let (commands, receiver) = mpsc::channel(1);
        let probes = Arc::new(Notify::new());
        let span = tracing::debug_span!("provider_websocket");
        let idle_from = (!busy).then_some(opened_at);
        let task = tokio::spawn(
            run(
                socket,
                receiver,
                Arc::clone(&shared),
                clock,
                limits,
                Arc::clone(&probes),
                idle_from,
            )
            .instrument(span),
        );
        Connection { commands, shared, opened_at, probes, task }
    }

    /// True when the connection is younger than `max_age` at `now`.
    pub(crate) fn is_younger(&self, now: Timestamp, max_age: Duration) -> bool {
        now.duration_since(self.opened_at).unsigned_abs() < max_age
    }

    /// Asks the task to check the connection with a ping when it had no request for
    /// [`Limits::probe_after`]: its pong makes the next request skip the ping, and a
    /// connection that gives none closes, so the next request opens a new one. A free
    /// connection only; one that runs a request shows its health in the answer.
    pub(crate) fn probe(&self) {
        self.probes.notify_one();
    }

    /// Marks the connection busy and takes its last completed answer, when it is open,
    /// free and younger than `max_age` at `now`. `None` when it cannot take a request.
    pub(crate) fn take(&self, now: Timestamp, max_age: Duration) -> Option<Option<Continuation>> {
        let mut shared = lock(&self.shared);
        let young = self.is_younger(now, max_age);
        if shared.busy || shared.closed || self.task.is_finished() || !young {
            return None;
        }
        shared.busy = true;
        Some(shared.last.take())
    }

    /// True while a request runs or drains.
    pub(crate) fn is_busy(&self) -> bool {
        lock(&self.shared).busy
    }

    /// True once the connection takes no more requests.
    pub(crate) fn is_closed(&self) -> bool {
        lock(&self.shared).closed || self.task.is_finished()
    }

    /// True once the task has ended. Until then, dropping the handle would stop a
    /// task that may still tell its caller how the last request ended.
    pub(crate) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Hands `create` to the task. False when the task has ended.
    pub(crate) async fn send(&self, create: Create) -> bool {
        self.commands.send(create).await.is_ok()
    }

    /// Frees a connection that was taken but got no request.
    pub(crate) fn release(&self) {
        lock(&self.shared).busy = false;
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How a request left the connection.
enum Served {
    /// Ready for the next request.
    Idle,
    /// The connection must close.
    Dead,
}

async fn run(
    mut socket: WebSocket,
    mut commands: mpsc::Receiver<Create>,
    shared: Arc<Mutex<Shared>>,
    clock: Arc<dyn Clock>,
    limits: Limits,
    probes: Arc<Notify>,
    idle_from: Option<Timestamp>,
) {
    // NOTE: `None` until the first request has ended when the caller that opened the
    // connection sends its request at once, on a socket that just answered. A
    // connection that a warm-up opened starts idle at its opening.
    let mut idle_since: Option<Timestamp> = idle_from;
    loop {
        // NOTE: the idle time runs from the end of the last request, or from the pong of
        // a warm-up's ping: a prompt follows that ping soon.
        let mut idle = clock.sleep(limits.idle);
        // NOTE: the open check of a warm-up: its ping went out, and its pong is on its
        // way. A request that comes before the pong takes it over.
        let mut probe: Option<Proof> = None;
        let create = loop {
            tokio::select! {
                create = commands.recv() => break create,
                message = socket.next() => {
                    // NOTE: nothing should arrive between answers; a close or an error
                    // here means the server dropped the connection while it was unused.
                    match message {
                        Some(Ok(WsMessage::Text(_) | WsMessage::Binary(_))) => {
                            tracing::debug!("the websocket sent a message between answers; closing it");
                        }
                        other => tracing::debug!(message = ?other, "the server closed an unused websocket"),
                    }
                    break None;
                }
                () = &mut idle => {
                    tracing::debug!(idle = ?limits.idle, "closing an idle websocket");
                    break None;
                }
                () = probes.notified(), if probe.is_none() => {
                    let quiet = idle_since
                        .is_some_and(|since| clock.now().duration_since(since).unsigned_abs() >= limits.probe_after);
                    if !quiet {
                        continue;
                    }
                    efr_stdx::mark::hit("ws.warm.ping");
                    match clock.timeout(limits.probe, socket.send_ping()).await {
                        Ok(Ok(pong)) => {
                            probe = Some(Proof { pong: Some(pong), deadline: clock.sleep(limits.probe) });
                        }
                        _ => {
                            tracing::debug!("the ping of a warm-up could not be sent; closing the websocket");
                            break None;
                        }
                    }
                }
                heard = listen(&mut probe), if probe.is_some() => {
                    if matches!(heard, Heard::Pong) {
                        efr_stdx::mark::hit("ws.warm.pong");
                        probe = None;
                        idle_since = Some(clock.now());
                        idle = clock.sleep(limits.idle);
                    } else {
                        tracing::debug!("a quiet websocket did not answer the ping of a warm-up; closing it");
                        break None;
                    }
                }
            }
        };
        let Some(create) = create else {
            break;
        };
        let quiet = match idle_since.map(|since| clock.now().duration_since(since).unsigned_abs()) {
            Some(idle) if idle >= limits.ping_first => Quiet::Long,
            Some(idle) if idle >= limits.probe_after => Quiet::Short,
            _ => Quiet::No,
        };
        match serve(&mut socket, create, &shared, &*clock, limits, quiet, probe).await {
            Served::Idle => idle_since = Some(clock.now()),
            Served::Dead => break,
        }
    }
    retire(&shared);
    let _ = clock.timeout(limits.send, socket.close(1000)).await;
}

/// How long a connection went without a request, as its next request checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quiet {
    /// Less than [`Limits::probe_after`]: the request goes out with no ping.
    No,
    /// At least [`Limits::probe_after`]: a ping goes out with the request.
    Short,
    /// At least [`Limits::ping_first`]: the pong must come before the request goes out.
    Long,
}

/// The open check of a quiet connection: its ping went out with the request, and the
/// pong or the first message shows that the connection still works.
struct Proof {
    /// The wait for the pong; `None` once the connection ended before it.
    pong: Option<Pong>,
    /// The end of [`Limits::probe`] from the ping.
    deadline: Sleep,
}

/// What came first while a [`Proof`] was open.
enum Heard {
    /// The pong: the connection works.
    Pong,
    /// The connection ended before the pong. The next read tells whether a message of
    /// the server came before the end.
    NoPong,
    /// Neither the pong nor a message came within [`Limits::probe`].
    Late,
}

/// Waits for the pong of `proof` or for its deadline; never ends without a proof.
async fn listen(proof: &mut Option<Proof>) -> Heard {
    let Some(Proof { pong, deadline }) = proof.as_mut() else {
        return std::future::pending().await;
    };
    let pong = async {
        match pong.as_mut() {
            Some(pong) => pong.await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        answered = pong => if answered.is_ok() { Heard::Pong } else { Heard::NoPong },
        () = deadline => Heard::Late,
    }
}

/// Ends the open check: the connection showed that it works.
fn prove(proof: &mut Option<Proof>) {
    if proof.take().is_some() {
        efr_stdx::mark::hit("ws.probe.end");
    }
}

/// Takes what a quiet connection showed. `Some` when the request must end: the
/// connection did not show in time that it works.
async fn hear(heard: Heard, proof: &mut Option<Proof>, request: &Request<'_>) -> Option<Served> {
    match heard {
        Heard::Pong => prove(proof),
        Heard::NoPong => {
            if let Some(proof) = proof.as_mut() {
                proof.pong = None;
            }
        }
        Heard::Late => {
            efr_stdx::mark::hit("ws.probe.late");
            let reason =
                "a quiet websocket answered neither its ping nor its request in time".to_owned();
            return Some(request.fail(reason, Health::Fine, ProviderError::Incomplete).await);
        }
    }
    None
}

/// Marks the connection closed and free: it takes no more requests.
fn retire(shared: &Mutex<Shared>) {
    let mut shared = lock(shared);
    shared.closed = true;
    shared.busy = false;
    shared.last = None;
}

/// The state of one request.
struct Request<'a> {
    events: &'a mpsc::Sender<Delivery>,
    shared: &'a Mutex<Shared>,
    /// The server took the request: an event of the answer arrived.
    accepted: bool,
    /// The answer's id, from its first event that carries it.
    response_id: Option<String>,
    /// The caller has dropped its stream.
    gone: bool,
    /// `response.interrupt` went out.
    interrupted: bool,
    /// The prints of the output items finished so far.
    items: Vec<Option<Print>>,
}

impl Request<'_> {
    /// Ends the request with `delivery` for the caller, when the caller still listens.
    async fn deliver(&self, delivery: Delivery) {
        if !self.gone {
            let _ = self.events.send(delivery).await;
        }
    }

    /// Marks the connection closed. It happens before the caller hears of the end, so
    /// the caller's next request opens a new connection instead of finding this one
    /// busy.
    fn retire(&self) {
        retire(self.shared);
    }

    /// Ends a request that failed with a refusal before the server took it, or with
    /// `failed` after. Only for a failure that shows that the server did not act on
    /// the request when no answer has started: the server's own close, a message that
    /// breaks the protocol, or a quiet connection that gave neither its pong nor a
    /// message.
    async fn fail(&self, reason: String, health: Health, failed: ProviderError) -> Served {
        self.retire();
        if self.accepted {
            self.deliver(Delivery::Failed(failed)).await;
        } else {
            self.deliver(Delivery::Refused(Refusal { reason, health })).await;
        }
        Served::Dead
    }

    /// Ends a request with `failed`, also before the server took it. For a failure
    /// after the request went out that does not show whether the server acted on it:
    /// a broken connection or a timeout. The server may then run the model, so the
    /// call must not go out again, as on the HTTP path (`efr_http::RetryPolicy`).
    async fn lose(&self, failed: ProviderError) -> Served {
        self.retire();
        self.deliver(Delivery::Failed(failed)).await;
        Served::Dead
    }
}

/// Runs one request. `quiet`: how long the connection had no request, so whether it
/// may have died without a close. `probe`: the open check of a warm-up whose pong has
/// not come yet; the request takes it over and sends no ping of its own.
async fn serve(
    socket: &mut WebSocket,
    create: Create,
    shared: &Mutex<Shared>,
    clock: &dyn Clock,
    limits: Limits,
    quiet: Quiet,
    probe: Option<Proof>,
) -> Served {
    let Create { message, settings, prints, events } = create;
    let mut request = Request {
        events: &events,
        shared,
        accepted: false,
        response_id: None,
        gone: false,
        interrupted: false,
        items: Vec::new(),
    };
    // NOTE: the ping of a quiet connection goes out first and the request right after
    // it, with no wait for the pong (Codex sends no ping at all). Until the pong or the
    // first message comes, a failure or the end of `limits.probe` sends the call over
    // HTTP: no event came, so the server started no answer. The other way, with no
    // ping, a connection that died without a close would hold the call for the read
    // timeout of 300 s. The rest risk: a connection that is only late, not dead, gets
    // the request after the call went over HTTP, so the model may run two times. After
    // a long quiet time, the likely case of a sleep of the laptop, the request waits
    // for the pong instead, and the request goes out only on a connection that works.
    let mut proof = None;
    if let Some(probe) = probe.filter(|_| quiet != Quiet::No) {
        // NOTE: the ping of a warm-up went out a moment ago. A short quiet connection
        // sends the request now and lets that pong prove the connection; a long quiet
        // one waits for that pong, as for a ping of its own.
        efr_stdx::mark::hit("ws.probe.begin");
        if quiet == Quiet::Short {
            proof = Some(probe);
        } else {
            let heard = listen(&mut Some(probe)).await;
            efr_stdx::mark::hit("ws.probe.end");
            if !matches!(heard, Heard::Pong) {
                let reason =
                    "a long quiet websocket did not answer the ping of its warm-up".to_owned();
                return request.fail(reason, Health::Fine, ProviderError::Incomplete).await;
            }
        }
    } else if quiet == Quiet::Long {
        efr_stdx::mark::hit("ws.probe.begin");
        let reason = match Clock::timeout(&clock, limits.probe, socket.ping()).await {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(format!("the ping of a long quiet websocket failed: {error}")),
            Err(_) => Some("a long quiet websocket did not answer its ping in time".to_owned()),
        };
        efr_stdx::mark::hit("ws.probe.end");
        if let Some(reason) = reason {
            // NOTE: the request has not gone out, so the call may go over HTTP.
            return request.fail(reason, Health::Fine, ProviderError::Incomplete).await;
        }
    }
    if quiet == Quiet::Short && proof.is_none() {
        efr_stdx::mark::hit("ws.probe.begin");
        let reason = match Clock::timeout(&clock, limits.probe, socket.send_ping()).await {
            Ok(Ok(pong)) => {
                proof = Some(Proof { pong: Some(pong), deadline: clock.sleep(limits.probe) });
                None
            }
            Ok(Err(error)) => Some(format!("the ping of a quiet websocket failed: {error}")),
            Err(_) => Some("the ping of a quiet websocket could not be sent in time".to_owned()),
        };
        if let Some(reason) = reason {
            // NOTE: the request has not gone out, so the call may go over HTTP. A dead
            // connection says nothing about the transport, so WebSockets do not pause.
            return request.fail(reason, Health::Fine, ProviderError::Incomplete).await;
        }
    }
    let sent = {
        let mut send =
            std::pin::pin!(Clock::timeout(&clock, limits.send, socket.send_text(message)));
        loop {
            tokio::select! {
                biased;
                sent = &mut send => break sent,
                heard = listen(&mut proof), if proof.is_some() => {
                    if let Some(served) = hear(heard, &mut proof, &request).await {
                        return served;
                    }
                }
            }
        }
    };
    match sent {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            // NOTE: the writer reports an error only for a frame that it could not
            // write whole, and the server acts on no part of a frame.
            let reason = format!("the request could not be sent: {error}");
            return request.fail(reason, Health::Fine, transport(error)).await;
        }
        Err(_) => {
            // NOTE: the frame stays in the writer's queue and may still go out.
            let url = socket.url().to_owned();
            return request.lose(transport(HttpError::Timeout { url })).await;
        }
    }
    let mut drain: Option<Sleep> = None;
    loop {
        let message = tokio::select! {
            biased;
            () = events.closed(), if !request.gone => {
                request.gone = true;
                drain = Some(clock.sleep(limits.drain));
                if !interrupt(socket, &mut request).await {
                    return Served::Dead;
                }
                continue;
            }
            () = async { if let Some(drain) = drain.as_mut() { drain.await } }, if drain.is_some() => {
                tracing::debug!("an interrupted answer did not end in time; closing the websocket");
                return Served::Dead;
            }
            message = Clock::timeout(&clock, limits.read, socket.next()) => message,
            heard = listen(&mut proof), if proof.is_some() => {
                if let Some(served) = hear(heard, &mut proof, &request).await {
                    return served;
                }
                continue;
            }
        };
        // NOTE: the pong and the end of the connection can be ready at once, and the
        // read above comes first. A pong that came before the end showed a live server
        // that has the request, so the end is then the call's failure.
        let answered = proof
            .as_mut()
            .and_then(|open| open.pong.as_mut())
            .and_then(futures::FutureExt::now_or_never);
        match answered {
            Some(Ok(())) => prove(&mut proof),
            Some(Err(_)) => {
                if let Some(open) = proof.as_mut() {
                    open.pong = None;
                }
            }
            None => {}
        }
        if proof.is_some() {
            // NOTE: a quiet connection that breaks or ends before its pong and before
            // any message was dead: the server started no answer.
            let ended = match &message {
                Ok(Some(Err(error))) => Some(format!("a quiet websocket broke: {error}")),
                Ok(None) => Some("a quiet websocket ended without a close".to_owned()),
                Err(_) => Some("a quiet websocket said nothing in time".to_owned()),
                Ok(Some(Ok(_))) => None,
            };
            if let Some(reason) = ended {
                return request.fail(reason, Health::Fine, ProviderError::Incomplete).await;
            }
        }
        let text = match message {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                prove(&mut proof);
                text
            }
            Ok(Some(Ok(WsMessage::Close { code, reason }))) => {
                let reason = format!("the server closed the websocket ({code:?} {reason})");
                return request.fail(reason, Health::Fine, ProviderError::Incomplete).await;
            }
            Ok(Some(Ok(_))) => {
                let reason = "the server sent a binary message".to_owned();
                let failed = ProviderError::InvalidStream { problem: "a binary websocket message" };
                return request.fail(reason, Health::Broken, failed).await;
            }
            Ok(Some(Err(error))) => return request.lose(transport(error)).await,
            Ok(None) => return request.lose(ProviderError::Incomplete).await,
            Err(_) => {
                let url = socket.url().to_owned();
                return request.lose(transport(HttpError::Timeout { url })).await;
            }
        };
        let event: Value = match serde_json::from_str(&text) {
            Ok(event) => event,
            Err(source) => {
                let reason = "the server sent a message that is not JSON".to_owned();
                return request
                    .fail(reason, Health::Broken, ProviderError::Decode { source })
                    .await;
            }
        };
        let kind = event.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
        if !request.accepted {
            if kind == "error" {
                return refuse(&request, &event).await;
            }
            request.accepted = is_answer(&kind);
        }
        if request.response_id.is_none()
            && let Some(id) = response_id(&event)
        {
            request.response_id = Some(id);
            if request.gone && !interrupt(socket, &mut request).await {
                return Served::Dead;
            }
        }
        if kind == "response.output_item.done"
            && let Some(item) = event.get("item")
        {
            request.items.push(Print::of(item));
        }
        if ENDS.contains(&kind.as_str()) {
            let healthy = matches!(kind.as_str(), "response.completed" | "response.incomplete");
            {
                // NOTE: the state changes before the caller sees the end, so the
                // caller's next request finds the connection free and continued.
                let mut shared = lock(shared);
                shared.busy = false;
                shared.closed = !healthy;
                if kind == "response.completed" && !request.gone {
                    shared.last = continuation(settings, prints, &mut request, &event);
                }
            }
            request.deliver(Delivery::Event(event)).await;
            return if healthy { Served::Idle } else { Served::Dead };
        }
        if !request.gone && events.send(Delivery::Event(event)).await.is_err() {
            request.gone = true;
            drain = Some(clock.sleep(limits.drain));
            if !interrupt(socket, &mut request).await {
                return Served::Dead;
            }
        }
    }
}

/// Ends a request whose first answer was an `error` event: the server refused it.
async fn refuse(request: &Request<'_>, event: &Value) -> Served {
    let error = event.get("error").filter(|error| error.is_object()).unwrap_or(event);
    let field = |key: &str| error.get(key).and_then(Value::as_str).unwrap_or_default();
    let code = field("code");
    let status = event
        .get("status")
        .or_else(|| event.get("status_code"))
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok());
    let reason = format!(
        "the server refused the request: status {status:?}, code {code:?}: {}",
        field("message")
    );
    request.retire();
    let delivery = match status {
        // NOTE: the server keeps no answer for this connection or ends it at its age
        // limit; a new connection or HTTP serves the call, and the socket is fine.
        _ if ROUTINE_ERRORS.contains(&code) => {
            Delivery::Refused(Refusal { reason, health: Health::Fine })
        }
        // NOTE: the HTTP path refreshes a refused token and sends the call again.
        Some(401) => Delivery::Refused(Refusal { reason, health: Health::Unauthorized }),
        // NOTE: an error status is the server's answer to the request itself, such as a
        // rate limit or a context overflow, as Codex reads it
        // (`map_wrapped_websocket_error_event`). HTTP would answer the same, so the call
        // fails at once: it does not go out twice, and WebSockets do not pause. A 408
        // says that the request did not arrive in time, which HTTP may still serve.
        Some(status) if (400..500).contains(&status) && status != 408 => {
            Delivery::Rejected(Rejection { status, event: event.clone() })
        }
        _ => Delivery::Refused(Refusal { reason, health: Health::Broken }),
    };
    request.deliver(delivery).await;
    Served::Dead
}

/// Sends `response.interrupt` for the answer when its id is known and it was not sent
/// yet. False when the message could not go out.
async fn interrupt(socket: &WebSocket, request: &mut Request<'_>) -> bool {
    let Some(id) = request.response_id.as_deref() else {
        return true;
    };
    if request.interrupted {
        return true;
    }
    request.interrupted = true;
    // NOTE: the message of Codex (`run_websocket_response_stream`): the partial items
    // of the answer are dropped, as an interrupted answer is on the HTTP path.
    let message = json!({
        "type": "response.interrupt",
        "response_id": id,
        "mode": "discard_partial_items",
    });
    tracing::debug!(response_id = %id, "interrupting the answer on the websocket");
    socket.send_text(message.to_string()).await.is_ok()
}

/// The continuation after a completed answer: the prints of the request's input and
/// of the answer's output items, from the finished items or else from the response's
/// `output`. `None` when an item has no print.
fn continuation(
    settings: ResponsesBody,
    prints: Option<Vec<Print>>,
    request: &mut Request<'_>,
    event: &Value,
) -> Option<Continuation> {
    let mut input = prints?;
    let response = event.get("response");
    let response_id = request.response_id.clone().or_else(|| {
        response.and_then(|response| response.get("id")).and_then(Value::as_str).map(str::to_owned)
    })?;
    let items = std::mem::take(&mut request.items);
    if items.is_empty() {
        let output = response.and_then(|response| response.get("output"));
        input.extend(Print::all(output.and_then(Value::as_array).map_or(&[], Vec::as_slice))?);
    } else {
        input.extend(items.into_iter().collect::<Option<Vec<_>>>()?);
    }
    Some(Continuation { settings, input, response_id })
}

/// True for an event of an answer: the server took the request.
fn is_answer(kind: &str) -> bool {
    kind.starts_with("response.")
}

/// The answer's id that `event` carries, as `response.id` or `response_id`.
fn response_id(event: &Value) -> Option<String> {
    event
        .get("response")
        .and_then(|response| response.get("id"))
        .or_else(|| event.get("response_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn transport(error: HttpError) -> ProviderError {
    ProviderError::Transport { source: Box::new(error) }
}
