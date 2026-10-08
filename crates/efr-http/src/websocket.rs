//! WebSocket connections over the client's own TLS stack.
//!
//! [`HttpClient::websocket`](crate::HttpClient::websocket) sends the opening handshake
//! of RFC 6455 as an HTTP/1.1 `GET` through reqwest, so the socket uses the same rustls
//! configuration, timeouts and `User-Agent` as every other request. reqwest hands over
//! the upgraded connection, and `fastwebsockets` reads and writes the frames on it.
//!
//! A [`WebSocket`] runs two small tasks: a reader that collects whole messages, answers
//! pings and hands each pong to its [`WebSocket::ping`], and a writer that sends the
//! frames in order. Its methods only talk to
//! those tasks over channels, so [`WebSocket::next`] can be cancelled at any point
//! without losing a frame: the frame parser itself is not safe to cancel, and it never
//! runs inside a caller's `select!`. Dropping the `WebSocket` stops both tasks, which
//! closes the connection.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use efr_stdx::rng::Rng;
use fastwebsockets::{
    FragmentCollectorRead, Frame, OpCode, Payload, Role, WebSocketError, WebSocketWrite,
    after_handshake_split,
};
use http::header::{
    CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use http::{HeaderMap, HeaderValue, StatusCode};
use sha1::{Digest as _, Sha1};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::{HttpError, HttpRequest, redact};

/// The GUID that RFC 6455 (section 1.3) appends to a key to make the accept value.
const ACCEPT_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// How many received messages wait for the caller before the reader stops reading, so a
/// slow caller slows the socket instead of filling memory.
const INCOMING: usize = 64;

/// How many frames wait for the writer.
const OUTGOING: usize = 16;

/// The largest message the reader accepts: a finished Responses event repeats the whole
/// output, so it is far above the default of a chat socket.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

type Upgraded = reqwest::Upgraded;

/// One message from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WsMessage {
    /// A text message, whole.
    Text(String),
    /// A binary message, whole.
    Binary(Bytes),
    /// The server closed the connection, with its close code and reason when it sent
    /// them. Nothing follows.
    Close {
        /// The close code, such as 1000 for a normal close.
        code: Option<u16>,
        /// The reason text, possibly empty.
        reason: String,
    },
}

/// An open WebSocket client connection.
///
/// Built by [`HttpClient::websocket`](crate::HttpClient::websocket). Messages arrive
/// through [`next`](WebSocket::next); text goes out through
/// [`send_text`](WebSocket::send_text). Dropping it closes the connection.
pub struct WebSocket {
    url: String,
    headers: HeaderMap,
    incoming: mpsc::Receiver<Result<WsMessage, HttpError>>,
    outgoing: mpsc::Sender<Outgoing>,
    pings: Arc<Mutex<Pings>>,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

/// The pings that wait for their pong. A ping's payload is its number, so a pong names
/// the ping it answers.
#[derive(Default)]
struct Pings {
    /// The number of the next ping.
    next: u64,
    /// The pings without a pong yet, by number. `None` once the reader has stopped, so
    /// no pong can come.
    waiting: Option<Vec<(u64, oneshot::Sender<()>)>>,
}

impl Pings {
    fn lock(pings: &Mutex<Pings>) -> std::sync::MutexGuard<'_, Pings> {
        pings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends the wait of ping `number` and of every older ping: a server may answer only
    /// the newest of several pings (RFC 6455, section 5.5.3).
    fn answer(&mut self, number: u64) {
        if let Some(waiting) = self.waiting.as_mut() {
            for (_, done) in waiting.extract_if(.., |(waits, _)| *waits <= number) {
                let _ = done.send(());
            }
        }
    }
}

/// A frame for the writer, with where to report the result.
struct Outgoing {
    frame: Frame<'static>,
    done: Option<oneshot::Sender<Result<(), WebSocketError>>>,
}

impl WebSocket {
    /// Sends `text` as one text message. Fails when the connection is closed or the
    /// write fails; the connection is then unusable.
    pub async fn send_text(&self, text: String) -> Result<(), HttpError> {
        let frame = Frame::text(Payload::Owned(text.into_bytes()));
        self.send(frame).await
    }

    /// Sends a close frame with `code` and waits until it is written. Messages that
    /// were on their way still arrive through [`next`](WebSocket::next).
    pub async fn close(&self, code: u16) -> Result<(), HttpError> {
        self.send(Frame::close(code, b"")).await
    }

    async fn send(&self, frame: Frame<'static>) -> Result<(), HttpError> {
        let (done, result) = oneshot::channel();
        let closed = || HttpError::WebSocketClosed { url: self.url.clone() };
        self.outgoing.send(Outgoing { frame, done: Some(done) }).await.map_err(|_| closed())?;
        match result.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(source)) => Err(HttpError::WebSocket { url: self.url.clone(), source }),
            Err(_) => Err(closed()),
        }
    }

    /// Sends a ping and waits for its pong, which shows that the server still reads and
    /// answers. Fails when the connection is closed or ends before the pong. The wait
    /// has no limit of its own: the caller bounds it.
    pub async fn ping(&self) -> Result<(), HttpError> {
        let (done, pong) = oneshot::channel();
        let number = {
            let mut pings = Pings::lock(&self.pings);
            let number = pings.next;
            pings.next += 1;
            match pings.waiting.as_mut() {
                Some(waiting) => {
                    // NOTE: a caller that stopped waiting leaves its entry behind.
                    waiting.retain(|(_, done)| !done.is_closed());
                    waiting.push((number, done));
                }
                None => return Err(HttpError::WebSocketClosed { url: self.url.clone() }),
            }
            number
        };
        let payload = Payload::Owned(number.to_be_bytes().to_vec());
        self.send(Frame::new(true, OpCode::Ping, None, payload)).await?;
        pong.await.map_err(|_| HttpError::WebSocketClosed { url: self.url.clone() })
    }

    /// The next message, or `None` once the connection has ended and every message was
    /// read. An error ends the connection. Safe to cancel: a message that was not
    /// returned stays for the next call.
    pub async fn next(&mut self) -> Option<Result<WsMessage, HttpError>> {
        self.incoming.recv().await
    }

    /// True when the reader has stopped: the server closed the connection, a read
    /// failed, or the caller dropped what it read. Messages it read before may still be
    /// waiting in [`next`](WebSocket::next).
    pub fn is_finished(&self) -> bool {
        self.reader.is_finished()
    }

    /// The headers of the server's `101 Switching Protocols` answer.
    pub fn response_headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The redacted URL of the connection.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for WebSocket {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

impl fmt::Debug for WebSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocket")
            .field("url", &self.url)
            .field("finished", &self.is_finished())
            .finish_non_exhaustive()
    }
}

/// The value a server must answer in `Sec-WebSocket-Accept` for the handshake key
/// `key` (RFC 6455, section 4.2.2): the SHA-1 of the key and a fixed GUID, in base64.
pub fn websocket_accept(key: &str) -> String {
    let mut hash = Sha1::new();
    hash.update(key.as_bytes());
    hash.update(ACCEPT_GUID.as_bytes());
    STANDARD.encode(hash.finalize())
}

/// A fresh handshake key: 16 random bytes in base64.
pub(crate) fn handshake_key(rng: &dyn Rng) -> String {
    let mut bytes = [0_u8; 16];
    rng.fill_bytes(&mut bytes);
    STANDARD.encode(bytes)
}

/// `request` with the headers of an opening handshake that offers `key`.
pub(crate) fn handshake_headers(request: &HttpRequest, key: &str) -> Result<HeaderMap, HttpError> {
    let mut headers = request.headers().clone();
    headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
    let key = HeaderValue::from_str(key)
        .map_err(|_| HttpError::InvalidHeaderValue { name: SEC_WEBSOCKET_KEY })?;
    headers.insert(SEC_WEBSOCKET_KEY, key);
    Ok(headers)
}

/// Checks the server's answer to a handshake that offered `key`: the status, the
/// `Upgrade` header and the accept value.
pub(crate) fn check_answer(
    url: &str,
    status: StatusCode,
    headers: &HeaderMap,
    key: &str,
) -> Result<(), HttpError> {
    if status != StatusCode::SWITCHING_PROTOCOLS {
        return Err(HttpError::UpgradeRefused { url: url.to_owned(), status });
    }
    let upgrade = headers.get(UPGRADE).and_then(|value| value.to_str().ok()).unwrap_or_default();
    if !upgrade.eq_ignore_ascii_case("websocket") {
        return Err(HttpError::Handshake {
            url: url.to_owned(),
            problem: "the answer does not upgrade to websocket",
        });
    }
    let accept = headers.get(SEC_WEBSOCKET_ACCEPT).and_then(|value| value.to_str().ok());
    if accept != Some(websocket_accept(key).as_str()) {
        return Err(HttpError::Handshake {
            url: url.to_owned(),
            problem: "the answer does not accept the handshake key",
        });
    }
    Ok(())
}

/// Starts the reader and the writer on an upgraded connection.
pub(crate) fn start(url: String, headers: HeaderMap, upgraded: Upgraded) -> WebSocket {
    let (read, write) = tokio::io::split(upgraded);
    let (mut read, write) = after_handshake_split(read, write, Role::Client);
    read.set_max_message_size(MAX_MESSAGE);
    let (incoming_tx, incoming) = mpsc::channel(INCOMING);
    let (outgoing, outgoing_rx) = mpsc::channel(OUTGOING);
    let pings = Arc::new(Mutex::new(Pings { next: 0, waiting: Some(Vec::new()) }));
    let reader = tokio::spawn({
        let read = FragmentCollectorRead::new(read);
        let (outgoing, url, pings) = (outgoing.clone(), url.clone(), Arc::clone(&pings));
        async move {
            read_loop(read, incoming_tx, outgoing, url, &pings).await;
            // NOTE: no pong can come now; dropping the waiters fails their pings.
            Pings::lock(&pings).waiting = None;
        }
    });
    let writer = tokio::spawn(write_loop(write, outgoing_rx));
    WebSocket { url, headers, incoming, outgoing, pings, reader, writer }
}

async fn read_loop(
    mut read: FragmentCollectorRead<ReadHalf<Upgraded>>,
    incoming: mpsc::Sender<Result<WsMessage, HttpError>>,
    outgoing: mpsc::Sender<Outgoing>,
    url: String,
    pings: &Mutex<Pings>,
) {
    // NOTE: the parser asks for the pong to a ping and the echo of a close through this
    // closure; the writer task sends them in order with everything else.
    let mut obligated = |frame: Frame<'static>| {
        let outgoing = outgoing.clone();
        async move {
            outgoing
                .send(Outgoing { frame, done: None })
                .await
                .map_err(|_| WebSocketError::ConnectionClosed)
        }
    };
    loop {
        let message = match read.read_frame(&mut obligated).await {
            Ok(frame) => match frame.opcode {
                OpCode::Text => match String::from_utf8(frame.payload.to_vec()) {
                    Ok(text) => Ok(WsMessage::Text(text)),
                    Err(_) => Err(HttpError::WebSocket {
                        url: url.clone(),
                        source: WebSocketError::InvalidUTF8,
                    }),
                },
                OpCode::Binary => Ok(WsMessage::Binary(Bytes::from(frame.payload.to_vec()))),
                OpCode::Close => Ok(close_message(&frame.payload)),
                OpCode::Pong => {
                    // NOTE: a pong that names no ping of ours is allowed as a heartbeat
                    // (RFC 6455, section 5.5.3) and answers nothing.
                    if let Ok(number) = <[u8; 8]>::try_from(&frame.payload[..]) {
                        Pings::lock(pings).answer(u64::from_be_bytes(number));
                    }
                    continue;
                }
                // Pings are answered by the parser; continuation frames carry nothing
                // for the caller.
                OpCode::Ping | OpCode::Continuation => continue,
            },
            Err(source) => Err(HttpError::WebSocket { url: url.clone(), source }),
        };
        let last = !matches!(message, Ok(WsMessage::Text(_) | WsMessage::Binary(_)));
        if incoming.send(message).await.is_err() || last {
            return;
        }
    }
}

async fn write_loop(
    mut write: WebSocketWrite<WriteHalf<Upgraded>>,
    mut outgoing: mpsc::Receiver<Outgoing>,
) {
    while let Some(Outgoing { frame, done }) = outgoing.recv().await {
        let mut result = write.write_frame(frame).await;
        if result.is_ok() {
            result = write.flush().await;
        }
        let failed = result.is_err();
        if let Some(done) = done {
            let _ = done.send(result);
        }
        if failed {
            return;
        }
    }
}

/// The close code and reason of a close frame's payload.
fn close_message(payload: &[u8]) -> WsMessage {
    match payload {
        [high, low, reason @ ..] => WsMessage::Close {
            code: Some(u16::from_be_bytes([*high, *low])),
            reason: String::from_utf8_lossy(reason).into_owned(),
        },
        _ => WsMessage::Close { code: None, reason: String::new() },
    }
}

/// The redacted form of `request`'s URL, for errors and logs.
pub(crate) fn redacted_url(request: &HttpRequest) -> String {
    redact::url(request.url())
}

#[cfg(test)]
mod tests;
