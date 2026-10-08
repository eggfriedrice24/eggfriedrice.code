//! A fake Responses server on the loopback interface that speaks both transports: the
//! WebSocket protocol as Codex expects it (one `response.create` text message per
//! request, the answer's events as text messages, `response.interrupt` while an answer
//! runs) and `POST /responses` with a server-sent event stream.
//!
//! Each WebSocket connection follows its own [`Socket`] script, in the order the
//! connections arrive; each `POST` gets the next body of the HTTP queue. The server
//! records what it receives, so a test can check the handshake headers and every
//! message.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use efr_http::websocket_accept;
use fastwebsockets::{Frame, OpCode, Payload, Role, WebSocket};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// What the server does with one request on a socket.
#[derive(Debug, Clone)]
pub(crate) enum Step {
    /// Sends these events.
    Send(Vec<Value>),
    /// Waits for the next message (the interrupt), records it, then sends these events.
    AfterNext(Vec<Value>),
    /// Closes the connection.
    Close,
    /// Drops the connection without a close frame, as a connection that breaks.
    Drop,
    /// Stops reading and sending but keeps the connection open, as a connection that
    /// died without a close: a ping gets no pong.
    Hang,
}

/// The script of one WebSocket connection.
#[derive(Debug, Clone)]
pub(crate) struct Socket {
    /// `None` accepts the handshake; a status refuses it.
    pub(crate) refuse: Option<u16>,
    /// The steps for each request, in order.
    pub(crate) requests: Vec<Vec<Step>>,
}

impl Socket {
    /// A connection that serves `requests`.
    pub(crate) fn serving(requests: Vec<Vec<Step>>) -> Socket {
        Socket { refuse: None, requests }
    }

    /// A handshake refused with `status`.
    pub(crate) fn refused(status: u16) -> Socket {
        Socket { refuse: Some(status), requests: Vec::new() }
    }
}

/// What one connection received.
#[derive(Debug, Clone, Default)]
pub(crate) struct SocketLog {
    /// The handshake headers, by lowercase name.
    pub(crate) headers: HashMap<String, String>,
    /// The request path.
    pub(crate) path: String,
    /// Every text message, as JSON.
    pub(crate) messages: Vec<Value>,
    /// The client closed the connection.
    pub(crate) closed: bool,
}

#[derive(Debug, Default)]
struct Log {
    sockets: Vec<SocketLog>,
    posts: Vec<Value>,
}

/// The running server.
#[derive(Debug)]
pub(crate) struct ResponsesServer {
    uri: String,
    log: Arc<Mutex<Log>>,
}

impl ResponsesServer {
    /// Starts a server with a script for each WebSocket connection and an SSE body for
    /// each `POST`. A connection or a `POST` beyond its script gets a 503.
    pub(crate) async fn start(sockets: Vec<Socket>, posts: Vec<String>) -> ResponsesServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Log::default()));
        let sockets = Arc::new(Mutex::new(VecDeque::from(sockets)));
        let posts = Arc::new(Mutex::new(VecDeque::from(posts)));
        let shared = Arc::clone(&log);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let log = Arc::clone(&shared);
                let sockets = Arc::clone(&sockets);
                let posts = Arc::clone(&posts);
                tokio::spawn(async move { serve(stream, log, sockets, posts).await });
            }
        });
        ResponsesServer { uri, log }
    }

    /// The server's base URL, such as `http://127.0.0.1:41234`.
    pub(crate) fn uri(&self) -> &str {
        &self.uri
    }

    /// What each WebSocket connection received so far.
    pub(crate) fn sockets(&self) -> Vec<SocketLog> {
        self.log.lock().unwrap().sockets.clone()
    }

    /// The body of each `POST` so far.
    pub(crate) fn posts(&self) -> Vec<Value> {
        self.log.lock().unwrap().posts.clone()
    }
}

/// The events of a server-sent event stream, as the JSON of each `data` line.
pub(crate) fn sse_events(sse: &str) -> Vec<Value> {
    sse.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty() && *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

struct Head {
    line: String,
    headers: HashMap<String, String>,
}

async fn read_head(stream: &mut TcpStream) -> Option<Head> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        if stream.read_exact(&mut byte).await.is_err() {
            return None;
        }
        bytes.push(byte[0]);
    }
    let text = String::from_utf8(bytes).ok()?;
    let mut lines = text.split("\r\n");
    let line = lines.next()?.to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some(Head { line, headers })
}

async fn serve(
    mut stream: TcpStream,
    log: Arc<Mutex<Log>>,
    sockets: Arc<Mutex<VecDeque<Socket>>>,
    posts: Arc<Mutex<VecDeque<String>>>,
) {
    let Some(head) = read_head(&mut stream).await else {
        return;
    };
    if head.line.starts_with("POST") {
        let length = head.headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).await.unwrap();
        log.lock().unwrap().posts.push(serde_json::from_slice(&body).unwrap());
        let reply = match posts.lock().unwrap().pop_front() {
            Some(sse) => format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                sse.len()
            ),
            None => unavailable(),
        };
        let _ = stream.write_all(reply.as_bytes()).await;
        return;
    }
    let script = sockets.lock().unwrap().pop_front();
    let path = head.line.split(' ').nth(1).unwrap_or_default().to_owned();
    let index = {
        let mut log = log.lock().unwrap();
        log.sockets.push(SocketLog { headers: head.headers.clone(), path, ..SocketLog::default() });
        log.sockets.len() - 1
    };
    let script = match script {
        Some(Socket { refuse: None, requests }) => requests,
        Some(Socket { refuse: Some(status), .. }) => {
            let reply = format!(
                "HTTP/1.1 {status} Refused\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
            let _ = stream.write_all(reply.as_bytes()).await;
            return;
        }
        None => {
            let _ = stream.write_all(unavailable().as_bytes()).await;
            return;
        }
    };
    let key = head.headers.get("sec-websocket-key").cloned().unwrap_or_default();
    let reply = format!(
        "HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: {}\r\n\r\n",
        websocket_accept(&key)
    );
    if stream.write_all(reply.as_bytes()).await.is_err() {
        return;
    }
    let mut socket = WebSocket::after_handshake(stream, Role::Server);
    let record = |message: Option<Value>| {
        let mut log = log.lock().unwrap();
        match message {
            Some(message) => log.sockets[index].messages.push(message),
            None => log.sockets[index].closed = true,
        }
    };
    for steps in script {
        let Some(create) = next_message(&mut socket).await else {
            record(None);
            return;
        };
        record(Some(create));
        for step in steps {
            match step {
                Step::Send(events) => {
                    if !send_all(&mut socket, &events).await {
                        return;
                    }
                }
                Step::AfterNext(events) => {
                    let Some(message) = next_message(&mut socket).await else {
                        record(None);
                        return;
                    };
                    record(Some(message));
                    if !send_all(&mut socket, &events).await {
                        return;
                    }
                }
                Step::Close => {
                    let _ = socket.write_frame(Frame::close(1011, b"going away")).await;
                    return;
                }
                Step::Drop => return,
                Step::Hang => {
                    let _socket = socket;
                    std::future::pending::<()>().await;
                    return;
                }
            }
        }
    }
    // Past its script, the connection records what else arrives until the client
    // closes it.
    loop {
        match next_message(&mut socket).await {
            Some(message) => record(Some(message)),
            None => {
                record(None);
                return;
            }
        }
    }
}

fn unavailable() -> String {
    "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
}

/// The next text message as JSON, or `None` once the client closed the connection.
async fn next_message(socket: &mut WebSocket<TcpStream>) -> Option<Value> {
    loop {
        let frame = socket.read_frame().await.ok()?;
        match frame.opcode {
            OpCode::Text => return Some(serde_json::from_slice(&frame.payload).unwrap()),
            OpCode::Close => return None,
            _ => {}
        }
    }
}

async fn send_all(socket: &mut WebSocket<TcpStream>, events: &[Value]) -> bool {
    for event in events {
        let frame = Frame::text(Payload::Owned(event.to_string().into_bytes()));
        if socket.write_frame(frame).await.is_err() {
            return false;
        }
    }
    true
}
