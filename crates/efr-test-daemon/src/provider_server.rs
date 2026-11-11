//! [`ProviderServer`]: one loopback server for the latency benches that speaks the
//! Responses API over HTTP and over a WebSocket, the Messages API, the model lists and
//! the token endpoint, with waits that a bench sets, and that notes when each thing
//! came.
//!
//! [`ResponsesServer`](crate::ResponsesServer) and [`MessagesServer`] answer at once
//! and whole, and have no WebSocket. This one can wait before the response headers,
//! between the headers and the first event, before the answer to a WebSocket upgrade
//! and before a pong; it can play a WebSocket that died without a close; it can close each connection after its answer, so the next call
//! needs a new one; it can fail one call; and it notes each connection, so a bench sees
//! whether efr reused one. It is the first fake WebSocket of the Responses API in the
//! repo: it answers each `response.create` message with the events of one answer.
//!
//! The paths: a base URL is `http://<addr>/<name>/v1`, where `<name>` is free. A `POST`
//! whose path ends in `/responses` or `/messages` is a model call; a `GET` of
//! `/responses` with an upgrade is a WebSocket; a `GET` whose path ends in `/models` is
//! a model list, answered by the rule of its key ([`ServerKnobs::models`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use efr_stdx::time::{Clock as _, SystemClock};
use fastwebsockets::{Frame, OpCode, Payload, Role, WebSocketError, after_handshake_split};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::{MessagesAnswer, MessagesServer, ResponsesAnswer, TestDaemonError};

/// The text of every answer.
pub const PROVIDER_TEXT: &str = "Hello. The build is fine.";

/// How the server answers the fetch of a model list with one key.
#[derive(Debug, Clone)]
pub struct ModelsRule {
    /// The status; 200 sends the list.
    pub status: u16,
    /// The wait before the answer.
    pub delay: Duration,
}

/// What a bench sets.
#[derive(Debug, Clone, Default)]
pub struct ServerKnobs {
    /// The wait before the response headers of a model call.
    pub headers: Duration,
    /// The wait between the headers and the first event.
    pub first_event: Duration,
    /// The wait before the answer to a WebSocket upgrade.
    pub upgrade: Duration,
    /// The wait before a pong. The connection reads and answers in that time, as a
    /// real server whose pong is on its way.
    pub pong: Duration,
    /// The WebSocket died without a close, as after a sleep of the laptop: from now
    /// on each connection reads every frame, answers none, not even a ping, and stays
    /// open. HTTP still answers.
    pub dead: bool,
    /// Close each connection after its answer: an HTTP connection after the response,
    /// a WebSocket after the answer's last event.
    pub close: bool,
    /// Refuse a WebSocket upgrade with a 404, as a server without WebSockets.
    pub no_websocket: bool,
    /// The rule of the model list for an `authorization` header; a key without a rule
    /// gets the list.
    pub models: HashMap<String, ModelsRule>,
    /// The Claude models of the list.
    pub claude: Vec<String>,
    /// The next model call gets one call of this tool with this input, not the text.
    pub tool_once: Option<(String, Value)>,
    /// The next model call over HTTP gets this status and an error body.
    pub fail_once: Option<u16>,
    /// Answer the fetch of an OpenAI model list with an empty list, not with a 404,
    /// so efrd does not ask again soon.
    pub openai_list: bool,
}

/// What came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saw {
    /// A new TCP connection.
    Accepted,
    /// The whole body of a model call over HTTP.
    Post,
    /// The response headers of a model call went out.
    Headers,
    /// The first event of an answer went out.
    FirstEvent,
    /// A WebSocket upgrade request.
    Upgrade,
    /// The 101 of an upgrade went out.
    Upgraded,
    /// A `response.create` on a WebSocket.
    Create,
    /// A ping.
    Ping,
    /// A pong went out.
    Pong,
    /// A fetch of a model list.
    Models,
    /// A request to the token endpoint.
    Token,
}

/// One entry of the server's log.
#[derive(Debug, Clone, Copy)]
pub struct Seen {
    /// When it came or went out.
    pub at: Instant,
    /// What it was.
    pub what: Saw,
    /// The bytes of a body or a message.
    pub bytes: usize,
}

#[derive(Debug, Default)]
struct Shared {
    knobs: Mutex<ServerKnobs>,
    log: Mutex<Vec<Seen>>,
    connections: AtomicU64,
}

impl Shared {
    fn knobs(&self) -> ServerKnobs {
        self.knobs.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The events of the next answer, as a server-sent event body: the tool call that
    /// waits, else the text.
    fn answer(&self, messages: bool) -> String {
        let tool = self.knobs.lock().unwrap_or_else(PoisonError::into_inner).tool_once.take();
        let call = format!("call_bench_{}", self.connections.fetch_add(0, Ordering::Relaxed));
        match (tool, messages) {
            (Some((name, input)), true) => MessagesAnswer::tool_use(&call, &name, &input).body,
            (Some((name, input)), false) => ResponsesAnswer::tool_call(&call, &name, &input).body,
            (None, true) => MessagesAnswer::text(PROVIDER_TEXT).body,
            (None, false) => ResponsesAnswer::text(PROVIDER_TEXT).body,
        }
    }

    fn note(&self, what: Saw, bytes: usize) {
        let seen = Seen { at: Instant::now(), what, bytes };
        self.log.lock().unwrap_or_else(PoisonError::into_inner).push(seen);
    }
}

/// The running server.
#[derive(Debug)]
pub struct ProviderServer {
    addr: std::net::SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ProviderServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ProviderServer {
    /// Starts the server on a free loopback port.
    ///
    /// # Errors
    ///
    /// The port cannot be bound.
    pub async fn start() -> Result<ProviderServer, TestDaemonError> {
        let io = |source| TestDaemonError::Io { what: "binding the provider server", source };
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(io)?;
        let addr = listener.local_addr().map_err(io)?;
        let shared = Arc::new(Shared::default());
        let serving = Arc::clone(&shared);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let _ = stream.set_nodelay(true);
                serving.connections.fetch_add(1, Ordering::Relaxed);
                serving.note(Saw::Accepted, 0);
                tokio::spawn(serve(stream, Arc::clone(&serving)));
            }
        });
        Ok(ProviderServer { addr, shared, task })
    }

    /// The base URL of the provider `name`, such as `http://127.0.0.1:4100/openai/v1`.
    pub fn base_url(&self, name: &str) -> String {
        format!("http://{}/{name}/v1", self.addr)
    }

    /// The issuer of the login: the server itself.
    pub fn issuer(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Changes the knobs.
    pub fn set(&self, change: impl FnOnce(&mut ServerKnobs)) {
        change(&mut self.shared.knobs.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// Takes the log so far.
    pub fn drain(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.shared.log.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The head of a request and what the read took of its body.
struct Head {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    rest: Vec<u8>,
}

/// Reads the head of the next request; `None` when the client closed the connection.
async fn read_head(stream: &mut TcpStream, carried: &mut Vec<u8>) -> Option<Head> {
    let mut bytes = std::mem::take(carried);
    let mut buffer = vec![0_u8; 16 * 1024];
    let end = loop {
        if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    };
    let rest = bytes.split_off(end + 4);
    let text = String::from_utf8(bytes).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.split('?').next()?.to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some(Head { method, path, headers, rest })
}

/// Reads the body of `head`, whose length its `content-length` gives.
async fn read_body(stream: &mut TcpStream, head: &mut Head) -> Option<Vec<u8>> {
    let length: usize =
        head.headers.get("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
    let mut body = std::mem::take(&mut head.rest);
    if body.len() < length {
        let have = body.len();
        body.resize(length, 0);
        stream.read_exact(&mut body[have..]).await.ok()?;
    } else {
        head.rest = body.split_off(length);
    }
    Some(body)
}

async fn sleep(wait: Duration) {
    if !wait.is_zero() {
        SystemClock.sleep(wait).await;
    }
}

/// The `sec-websocket-accept` of the handshake key `key` (RFC 6455, section 4.2.2).
fn websocket_accept(key: &str) -> String {
    use sha1::Digest as _;
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hash = sha1::Sha1::new();
    hash.update(key.as_bytes());
    hash.update(GUID.as_bytes());
    STANDARD.encode(hash.finalize())
}

/// A whole response with a body of a known length.
fn response(status: u16, kind: &str, body: &str, close: bool) -> String {
    let connection = if close { "close" } else { "keep-alive" };
    format!(
        "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: {connection}\r\n\r\n{body}",
        body.len()
    )
}

async fn serve(mut stream: TcpStream, shared: Arc<Shared>) {
    let mut carried = Vec::new();
    loop {
        let Some(mut head) = read_head(&mut stream, &mut carried).await else {
            return;
        };
        let knobs = shared.knobs();
        let upgrade = head.headers.get("upgrade").is_some_and(|value| value == "websocket");
        if head.method == "GET" && upgrade {
            shared.note(Saw::Upgrade, 0);
            if knobs.no_websocket {
                let _ = stream.write_all(response(404, "text/plain", "", true).as_bytes()).await;
                return;
            }
            sleep(knobs.upgrade).await;
            let key = head.headers.get("sec-websocket-key").cloned().unwrap_or_default();
            let reply = format!(
                "HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: {}\r\n\r\n",
                websocket_accept(&key)
            );
            if stream.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
            shared.note(Saw::Upgraded, 0);
            socket(stream, Arc::clone(&shared)).await;
            return;
        }
        let Some(body) = read_body(&mut stream, &mut head).await else {
            return;
        };
        carried = std::mem::take(&mut head.rest);
        let model_call = head.method == "POST"
            && (head.path.ends_with("/responses") || head.path.ends_with("/messages"));
        let failed = if model_call {
            shared.knobs.lock().unwrap_or_else(PoisonError::into_inner).fail_once.take()
        } else {
            None
        };
        if let Some(status) = failed {
            shared.note(Saw::Post, body.len());
            let error = json!({
                "type": "error",
                "error": {"type": "overloaded_error", "message": "the bench is overloaded"},
            });
            let reply = response(status, "application/json", &error.to_string(), false);
            if stream.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        if model_call {
            shared.note(Saw::Post, body.len());
            let events = shared.answer(head.path.ends_with("/messages"));
            sleep(knobs.headers).await;
            let connection = if knobs.close { "close" } else { "keep-alive" };
            let headers = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nrequest-id: req_bench\r\nconnection: {connection}\r\n\r\n"
            );
            if stream.write_all(headers.as_bytes()).await.is_err() {
                return;
            }
            shared.note(Saw::Headers, 0);
            sleep(knobs.first_event).await;
            let chunk = format!("{:x}\r\n{events}\r\n0\r\n\r\n", events.len());
            shared.note(Saw::FirstEvent, events.len());
            if stream.write_all(chunk.as_bytes()).await.is_err() || knobs.close {
                return;
            }
            continue;
        }
        let reply = if head.path.ends_with("/models") {
            shared.note(Saw::Models, 0);
            let key = head.headers.get("authorization").cloned().unwrap_or_default();
            let anthropic = head.headers.contains_key("anthropic-version");
            match knobs.models.get(&key) {
                Some(rule) if rule.status != 200 => {
                    sleep(rule.delay).await;
                    let body = json!({"type": "error", "error": {"type": "authentication_error", "message": "the bench refused this key"}});
                    response(rule.status, "application/json", &body.to_string(), false)
                }
                rule => {
                    sleep(rule.map_or(Duration::ZERO, |rule| rule.delay)).await;
                    if anthropic {
                        let models: Vec<Value> =
                            knobs.claude.iter().map(|id| MessagesServer::model(id)).collect();
                        let id = |model: Option<&Value>| {
                            model.map_or(Value::Null, |model| model["id"].clone())
                        };
                        let body = json!({
                            "data": models, "has_more": false,
                            "first_id": id(models.first()), "last_id": id(models.last()),
                        });
                        response(200, "application/json", &body.to_string(), false)
                    } else if knobs.openai_list {
                        let body = json!({"object": "list", "data": []});
                        response(200, "application/json", &body.to_string(), false)
                    } else {
                        // NOTE: as `ResponsesServer` without a catalog: efrd keeps the
                        // built-in list.
                        response(404, "text/plain", "no catalog here", false)
                    }
                }
            }
        } else if head.path.ends_with("/oauth/token") {
            shared.note(Saw::Token, body.len());
            response(599, "text/plain", "the bench has no token answer", false)
        } else {
            response(404, "text/plain", "", false)
        };
        if stream.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// The JSON of each event of a server-sent event body.
fn events_of(sse: &str) -> Vec<Value> {
    sse.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

/// Serves `response.create` messages on an upgraded connection until the client closes
/// it, or until one answer when the knobs say to close. A writer task sends the frames,
/// so a pong that waits holds back no read and no answer, as on a real server.
async fn socket(stream: TcpStream, shared: Arc<Shared>) {
    let (read, write) = stream.into_split();
    let (mut read, mut write) = after_handshake_split(read, write, Role::Server);
    read.set_auto_pong(false);
    read.set_auto_close(true);
    let (frames, mut queue) = mpsc::unbounded_channel::<Frame<'static>>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = queue.recv().await {
            if write.write_frame(frame).await.is_err() || write.flush().await.is_err() {
                return;
            }
        }
    });
    // NOTE: the parser hands the echo of a close to this closure.
    let mut obligated = {
        let frames = frames.clone();
        move |frame: Frame<'static>| {
            let sent = frames.send(frame).map_err(|_| WebSocketError::ConnectionClosed);
            async move { sent }
        }
    };
    loop {
        let Ok(frame) = read.read_frame(&mut obligated).await else {
            break;
        };
        let knobs = shared.knobs();
        match frame.opcode {
            OpCode::Text | OpCode::Ping if knobs.dead => {}
            OpCode::Text => {
                shared.note(Saw::Create, frame.payload.len());
                let events = events_of(&shared.answer(false));
                sleep(knobs.headers + knobs.first_event).await;
                shared.note(Saw::FirstEvent, 0);
                for event in &events {
                    let frame = Frame::text(Payload::Owned(event.to_string().into_bytes()));
                    if frames.send(frame).is_err() {
                        break;
                    }
                }
                if knobs.close {
                    let _ = frames.send(Frame::close(1000, b"bench"));
                    break;
                }
            }
            OpCode::Ping => {
                shared.note(Saw::Ping, 0);
                let pong = Frame::pong(Payload::Owned(frame.payload.to_vec()));
                let (frames, shared) = (frames.clone(), Arc::clone(&shared));
                tokio::spawn(async move {
                    sleep(knobs.pong).await;
                    if frames.send(pong).is_ok() {
                        shared.note(Saw::Pong, 0);
                    }
                });
            }
            OpCode::Close => break,
            _ => {}
        }
    }
    drop((frames, obligated));
    let _ = writer.await;
}
