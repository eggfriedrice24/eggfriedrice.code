//! One Responses WebSocket connection and the task that owns it.
//!
//! The task sends one `response.create` at a time and passes the server's events to the
//! caller of that request. It keeps reading after the caller has gone: it then sends
//! `response.interrupt` for the answer and reads until the answer ends, so the
//! connection is clean for the next request. While no request runs, it closes the
//! connection after [`Limits::idle`] or when the server closes it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_http::{HttpError, WebSocket, WsMessage};
use efr_provider::ProviderError;
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::Instrument as _;

use super::continuation::Continuation;
use crate::convert::ResponsesBody;

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
}

/// Why the server did not take a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// What happened, for the log.
    pub(crate) reason: String,
    /// True when the failure says that WebSockets do not work now, so the provider
    /// pauses them; false for a stale connection or a routine error.
    pub(crate) pause: bool,
}

/// One request for the task.
#[derive(Debug)]
pub(crate) struct Create {
    /// The `response.create` message.
    pub(crate) message: String,
    /// Every field of the request but `input`.
    pub(crate) settings: ResponsesBody,
    /// The request's whole input.
    pub(crate) input: Vec<Value>,
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
    task: JoinHandle<()>,
}

impl Connection {
    /// Starts the task on `socket`, opened at `opened_at`. The connection starts busy:
    /// the caller that opened it sends the first request.
    pub(crate) fn start(
        socket: WebSocket,
        clock: Arc<dyn Clock>,
        limits: Limits,
        opened_at: Timestamp,
    ) -> Connection {
        let shared = Arc::new(Mutex::new(Shared { busy: true, ..Shared::default() }));
        let (commands, receiver) = mpsc::channel(1);
        let span = tracing::debug_span!("provider_websocket");
        let task = tokio::spawn(
            run(socket, receiver, Arc::clone(&shared), clock, limits).instrument(span),
        );
        Connection { commands, shared, opened_at, task }
    }

    /// Marks the connection busy and takes its last completed answer, when it is open,
    /// free and younger than `max_age` at `now`. `None` when it cannot take a request.
    pub(crate) fn take(&self, now: Timestamp, max_age: Duration) -> Option<Option<Continuation>> {
        let mut shared = lock(&self.shared);
        let young = now.duration_since(self.opened_at).unsigned_abs() < max_age;
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
) {
    loop {
        let create = tokio::select! {
            create = commands.recv() => create,
            message = socket.next() => {
                // NOTE: nothing should arrive between answers; a close or an error here
                // means the server dropped the connection while it was unused.
                match message {
                    Some(Ok(WsMessage::Text(_) | WsMessage::Binary(_))) => {
                        tracing::debug!("the websocket sent a message between answers; closing it");
                    }
                    other => tracing::debug!(message = ?other, "the server closed an unused websocket"),
                }
                None
            }
            () = clock.sleep(limits.idle) => {
                tracing::debug!(idle = ?limits.idle, "closing an idle websocket");
                None
            }
        };
        let Some(create) = create else {
            break;
        };
        match serve(&mut socket, create, &shared, &*clock, limits).await {
            Served::Idle => {}
            Served::Dead => break,
        }
    }
    {
        let mut shared = lock(&shared);
        shared.closed = true;
        shared.busy = false;
        shared.last = None;
    }
    let _ = clock.timeout(limits.send, socket.close(1000)).await;
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
    /// The output items finished so far.
    items: Vec<Value>,
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
        let mut shared = lock(self.shared);
        shared.closed = true;
        shared.busy = false;
        shared.last = None;
    }

    /// Ends a request that failed with a refusal before the server took it, or with
    /// `failed` after.
    async fn fail(&self, reason: String, pause: bool, failed: ProviderError) -> Served {
        self.retire();
        if self.accepted {
            self.deliver(Delivery::Failed(failed)).await;
        } else {
            self.deliver(Delivery::Refused(Refusal { reason, pause })).await;
        }
        Served::Dead
    }
}

async fn serve(
    socket: &mut WebSocket,
    create: Create,
    shared: &Mutex<Shared>,
    clock: &dyn Clock,
    limits: Limits,
) -> Served {
    let Create { message, settings, input, events } = create;
    let mut request = Request {
        events: &events,
        shared,
        accepted: false,
        response_id: None,
        gone: false,
        interrupted: false,
        items: Vec::new(),
    };
    match Clock::timeout(&clock, limits.send, socket.send_text(message)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let reason = format!("the request could not be sent: {error}");
            return request.fail(reason, false, transport(error)).await;
        }
        Err(_) => {
            let reason = "the request could not be sent in time".to_owned();
            return request.fail(reason, true, ProviderError::Incomplete).await;
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
        };
        let text = match message {
            Ok(Some(Ok(WsMessage::Text(text)))) => text,
            Ok(Some(Ok(WsMessage::Close { code, reason }))) => {
                let reason = format!("the server closed the websocket ({code:?} {reason})");
                return request.fail(reason, false, ProviderError::Incomplete).await;
            }
            Ok(Some(Ok(_))) => {
                let reason = "the server sent a binary message".to_owned();
                let failed = ProviderError::InvalidStream { problem: "a binary websocket message" };
                return request.fail(reason, true, failed).await;
            }
            Ok(Some(Err(error))) => {
                let reason = format!("the websocket failed: {error}");
                return request.fail(reason, false, transport(error)).await;
            }
            Ok(None) => {
                let reason = "the websocket ended".to_owned();
                return request.fail(reason, false, ProviderError::Incomplete).await;
            }
            Err(_) => {
                let url = socket.url().to_owned();
                let reason = "the server sent nothing in time".to_owned();
                return request.fail(reason, false, transport(HttpError::Timeout { url })).await;
            }
        };
        let event: Value = match serde_json::from_str(&text) {
            Ok(event) => event,
            Err(source) => {
                let reason = "the server sent a message that is not JSON".to_owned();
                return request.fail(reason, true, ProviderError::Decode { source }).await;
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
            request.items.push(item.clone());
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
                    shared.last = continuation(settings, input, &mut request, &event);
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
    let status = event.get("status").or_else(|| event.get("status_code")).and_then(Value::as_u64);
    let reason = format!(
        "the server refused the request: status {status:?}, code {code:?}: {}",
        field("message")
    );
    // NOTE: a refused token or a rate limit is not the socket's fault: the HTTP path
    // answers them the same way, and the next call tries the socket again.
    let pause = !ROUTINE_ERRORS.contains(&code) && !matches!(status, Some(401 | 429));
    request.retire();
    request.deliver(Delivery::Refused(Refusal { reason, pause })).await;
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

/// The continuation after a completed answer: the request's input and the answer's
/// output items, from the finished items or else from the response's `output`.
fn continuation(
    settings: ResponsesBody,
    mut input: Vec<Value>,
    request: &mut Request<'_>,
    event: &Value,
) -> Option<Continuation> {
    let response = event.get("response");
    let response_id = request.response_id.clone().or_else(|| {
        response.and_then(|response| response.get("id")).and_then(Value::as_str).map(str::to_owned)
    })?;
    let mut items = std::mem::take(&mut request.items);
    if items.is_empty() {
        items = response
            .and_then(|response| response.get("output"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
    }
    input.extend(items);
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
