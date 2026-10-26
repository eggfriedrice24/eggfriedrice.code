//! The WebSocket entry over an in-memory pipe, driven by a raw client that encodes and
//! decodes its frames by hand, so the tests check the bytes on the wire and not
//! fastwebsockets against itself.

use std::future::pending;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_protocol::framing;
use efr_protocol::{
    ClientFrame, ErrorCode, HelloResult, Origin, PROTOCOL_VERSION, RequestId, ServerFrame,
};
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::keepalive::PING_INTERVAL;
use super::{WebSocketOptions, frames, serve_websocket};
use crate::codec::EncodedFrame;
use crate::connection::{self, ConnectionParams, FrameSink as _, FrameSource as _};
use crate::testing::{
    FakeDispatcher, InstantClock, StoppedClock, TestClient, cancel_frame, hello_frame, internal,
    list_frame, subscribe_frame,
};
use crate::{
    ConnId, PeerCred, RemotePeer, Request, TransportError, WebSocketCloseReason, WebSocketRefusal,
};

const ORIGIN: &str = "http://100.64.0.1:7878";
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

const TEXT: u8 = 0x1;
const BINARY: u8 = 0x2;
const CONTINUATION: u8 = 0x0;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
const PONG: u8 = 0xa;

fn peer_addr() -> SocketAddr {
    "100.64.0.9:50123".parse().unwrap()
}

fn options() -> WebSocketOptions {
    WebSocketOptions::new().allow_origin(ORIGIN)
}

fn upgrade_request(origin: Option<&str>) -> Vec<u8> {
    let mut head = format!(
        "GET / HTTP/1.1\r\nHost: 100.64.0.1:7878\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {KEY}\r\nSec-WebSocket-Version: 13\r\n"
    );
    if let Some(origin) = origin {
        head.push_str(&format!("Origin: {origin}\r\n"));
    }
    head.push_str("\r\n");
    head.into_bytes()
}

/// A client frame, masked with [`MASK`] unless `masked` is false.
fn client_frame(fin: bool, opcode: u8, payload: &[u8], masked: bool) -> Vec<u8> {
    let mut out = vec![(u8::from(fin) << 7) | opcode];
    let mask_bit = if masked { 0x80 } else { 0 };
    match payload.len() {
        len if len < 126 => out.push(mask_bit | u8::try_from(len).unwrap()),
        len if len < 65_536 => {
            out.push(mask_bit | 126);
            out.extend_from_slice(&u16::try_from(len).unwrap().to_be_bytes());
        }
        len => {
            out.push(mask_bit | 127);
            out.extend_from_slice(&u64::try_from(len).unwrap().to_be_bytes());
        }
    }
    if masked {
        out.extend_from_slice(&MASK);
        out.extend(payload.iter().enumerate().map(|(n, byte)| byte ^ MASK[n % 4]));
    } else {
        out.extend_from_slice(payload);
    }
    out
}

fn text(frame: &ClientFrame) -> Vec<u8> {
    client_frame(true, TEXT, &serde_json::to_vec(frame).unwrap(), true)
}

/// One frame from the server.
#[derive(Debug, PartialEq, Eq)]
enum Received {
    Text(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Option<u16>, String),
    Other { fin: bool, opcode: u8 },
}

/// A raw WebSocket client on one end of the pipe.
struct Client {
    stream: DuplexStream,
}

impl Client {
    async fn send_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
        self.stream.flush().await.unwrap();
    }

    async fn send(&mut self, frame: &ClientFrame) {
        self.send_raw(&text(frame)).await;
    }

    /// The next frame, or `None` once the server closed the pipe.
    async fn next(&mut self) -> Option<Received> {
        let mut head = [0_u8; 2];
        self.stream.read_exact(&mut head).await.ok()?;
        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0f;
        assert_eq!(head[1] & 0x80, 0, "a server frame must not be masked");
        let len = match head[1] & 0x7f {
            126 => usize::from(self.stream.read_u16().await.unwrap()),
            127 => usize::try_from(self.stream.read_u64().await.unwrap()).unwrap(),
            len => usize::from(len),
        };
        let mut payload = vec![0; len];
        self.stream.read_exact(&mut payload).await.unwrap();
        Some(match (fin, opcode) {
            (true, TEXT) => Received::Text(payload),
            (true, PING) => Received::Ping(payload),
            (true, PONG) => Received::Pong(payload),
            (true, CLOSE) => match payload.as_slice() {
                [high, low, reason @ ..] => Received::Close(
                    Some(u16::from_be_bytes([*high, *low])),
                    String::from_utf8(reason.to_vec()).unwrap(),
                ),
                _ => Received::Close(None, String::new()),
            },
            (fin, opcode) => Received::Other { fin, opcode },
        })
    }

    /// The next server frame; anything else fails the test.
    async fn recv(&mut self) -> ServerFrame {
        match self.next().await {
            Some(Received::Text(payload)) => ServerFrame::from_json(&payload).unwrap(),
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    /// The close code of the close frame that comes next, after which the pipe ends.
    async fn close_code(&mut self) -> u16 {
        let code = match self.next().await {
            Some(Received::Close(Some(code), _)) => code,
            other => panic!("expected a close frame, got {other:?}"),
        };
        assert_eq!(self.next().await, None, "nothing follows a close frame");
        code
    }

    async fn hello(&mut self) -> HelloResult {
        self.send(&hello_frame(1, PROTOCOL_VERSION)).await;
        let ServerFrame::Item { id, item } = self.recv().await else {
            panic!("hello was not answered with an item");
        };
        assert_eq!(id, RequestId::new(1));
        assert_eq!(self.recv().await, ServerFrame::end(RequestId::new(1)));
        serde_json::from_value(item).unwrap()
    }
}

/// Reads an HTTP response head and returns its status and text.
async fn response_head(stream: &mut DuplexStream) -> (u16, String) {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let byte = stream.read_u8().await.unwrap();
        head.push(byte);
    }
    let text = String::from_utf8(head).unwrap();
    let status = text.split(' ').nth(1).unwrap().parse().unwrap();
    (status, text)
}

struct Served {
    task: JoinHandle<Result<(), TransportError>>,
    shutdown: CancellationToken,
}

/// Runs the WebSocket entry on one end of a pipe and returns the other end.
fn serve(dispatcher: Arc<FakeDispatcher>, clock: Arc<dyn Clock>) -> (DuplexStream, Served) {
    let (client, server) = tokio::io::duplex(1 << 16);
    let shutdown = CancellationToken::new();
    let peer = RemotePeer::new(peer_addr(), Arc::new("admitted"));
    let token = shutdown.clone();
    let task = tokio::spawn(async move {
        serve_websocket(server, peer, &options(), dispatcher, clock, token).await
    });
    (client, Served { task, shutdown })
}

/// A served connection whose handshake the client finished.
async fn connected(dispatcher: Arc<FakeDispatcher>, clock: Arc<dyn Clock>) -> (Client, Served) {
    let (mut stream, served) = serve(dispatcher, clock);
    stream.write_all(&upgrade_request(Some(ORIGIN))).await.unwrap();
    let (status, head) = response_head(&mut stream).await;
    assert_eq!(status, 101, "{head}");
    (Client { stream }, served)
}

/// Answers a unary request with `{"ok": true}`.
fn answering() -> Arc<FakeDispatcher> {
    FakeDispatcher::new(|request: Request| async move {
        request.responder.item(&json!({ "ok": true })).await.map_err(internal)
    })
}

fn id(value: u64) -> RequestId {
    RequestId::new(value)
}

#[tokio::test]
async fn an_upgrade_answers_with_the_accept_value_and_serves_json_frames_as_text() {
    let dispatcher = answering();
    let (mut client, served) = connected(Arc::clone(&dispatcher), Arc::new(StoppedClock)).await;
    let result = client.hello().await;
    assert_eq!(result.protocol, PROTOCOL_VERSION);
    client.send(&list_frame(2)).await;
    let item = client.next().await.unwrap();
    let expected = ServerFrame::Item { id: id(2), item: json!({ "ok": true }) };
    // The message is the bare JSON body of the frame, without the length prefix.
    assert_eq!(
        item,
        Received::Text(framing::encode(&expected).unwrap()[framing::HEADER_LEN..].to_vec())
    );
    assert_eq!(client.recv().await, ServerFrame::end(id(2)));
    drop(client);
    assert!(served.task.await.unwrap().is_ok());
}

#[tokio::test]
async fn the_101_answer_carries_the_accept_value_of_the_key() {
    let (mut stream, _served) = serve(answering(), Arc::new(StoppedClock));
    stream.write_all(&upgrade_request(None)).await.unwrap();
    let (status, head) = response_head(&mut stream).await;
    assert_eq!(status, 101);
    assert!(head.contains("\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"), "{head}");
    assert!(head.contains("\r\nUpgrade: websocket\r\n"), "{head}");
    assert!(!head.to_ascii_lowercase().contains("sec-websocket-extensions"), "{head}");
}

#[tokio::test]
async fn a_remote_context_is_a_phone_with_the_peer_address_and_the_admission_value() {
    let dispatcher = answering();
    let (mut client, _served) = connected(Arc::clone(&dispatcher), Arc::new(StoppedClock)).await;
    // The hello of the test client declares the cli.
    client.hello().await;
    let contexts = dispatcher.contexts();
    assert_eq!(contexts.len(), 1);
    let context = &contexts[0];
    assert_eq!(context.surface(), Origin::Phone);
    assert_eq!((context.uid(), context.pid()), (None, None));
    let remote = context.remote().unwrap();
    assert_eq!(remote.addr(), peer_addr());
    assert_eq!(remote.admission::<&str>(), Some(&"admitted"));
}

#[tokio::test]
async fn frames_sent_with_the_head_are_served() {
    let (mut stream, _served) = serve(answering(), Arc::new(StoppedClock));
    let mut bytes = upgrade_request(Some(ORIGIN));
    bytes.extend_from_slice(&text(&hello_frame(1, PROTOCOL_VERSION)));
    bytes.extend_from_slice(&text(&list_frame(2)));
    stream.write_all(&bytes).await.unwrap();
    assert_eq!(response_head(&mut stream).await.0, 101);
    let mut client = Client { stream };
    assert!(matches!(client.recv().await, ServerFrame::Item { id, .. } if id == RequestId::new(1)));
    assert_eq!(client.recv().await, ServerFrame::end(id(1)));
    assert_eq!(client.recv().await, ServerFrame::Item { id: id(2), item: json!({ "ok": true }) });
}

/// Sends `request`, reads the whole answer and returns it with the entry's result.
async fn refused(request: &[u8]) -> (u16, String, Result<(), TransportError>) {
    let dispatcher = answering();
    let (mut stream, served) = serve(Arc::clone(&dispatcher), Arc::new(StoppedClock));
    stream.write_all(request).await.unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    let text = String::from_utf8(answer).unwrap();
    let status = text.split(' ').nth(1).unwrap().parse().unwrap();
    let result = served.task.await.unwrap();
    assert!(dispatcher.contexts().is_empty(), "no hello can reach the dispatcher");
    (status, text, result)
}

fn refusal_of(result: Result<(), TransportError>) -> WebSocketRefusal {
    match result {
        Err(TransportError::HandshakeRefused { refusal }) => refusal,
        other => panic!("expected a refused handshake, got {other:?}"),
    }
}

#[tokio::test]
async fn a_plain_http_request_gets_an_error_and_never_200() {
    let (status, text, result) = refused(b"GET / HTTP/1.1\r\nHost: 100.64.0.1:7878\r\n\r\n").await;
    assert_eq!(status, 426, "{text}");
    assert!(text.ends_with("only WebSocket upgrades are served\n"), "{text}");
    assert_eq!(refusal_of(result), WebSocketRefusal::NotUpgrade);
}

#[tokio::test]
async fn a_web_page_origin_is_forbidden() {
    let (status, _, result) = refused(&upgrade_request(Some("http://evil.example"))).await;
    assert_eq!(status, 403);
    assert_eq!(refusal_of(result), WebSocketRefusal::Origin);
    let (status, _, result) = refused(&upgrade_request(Some("null"))).await;
    assert_eq!(status, 403);
    assert_eq!(refusal_of(result), WebSocketRefusal::Origin);
}

#[tokio::test]
async fn another_method_or_path_is_refused() {
    let post = String::from_utf8(upgrade_request(None)).unwrap().replacen("GET", "POST", 1);
    let (status, text, result) = refused(post.as_bytes()).await;
    assert_eq!(status, 405);
    assert!(text.contains("\r\nAllow: GET\r\n"), "{text}");
    assert_eq!(refusal_of(result), WebSocketRefusal::Method);
    let query = String::from_utf8(upgrade_request(None)).unwrap().replacen("GET /", "GET /?t=x", 1);
    let (status, _, result) = refused(query.as_bytes()).await;
    assert_eq!(status, 404);
    assert_eq!(refusal_of(result), WebSocketRefusal::Path);
}

#[tokio::test]
async fn a_head_larger_than_8_kib_is_refused_without_reading_on() {
    let mut request = b"GET / HTTP/1.1\r\nHost: x\r\nX-Pad: ".to_vec();
    request.extend(std::iter::repeat_n(b'a', 9000));
    let dispatcher = answering();
    let (mut stream, served) = serve(dispatcher, Arc::new(StoppedClock));
    // NOTE: the server stops reading at 8 KiB and closes, so the rest of this write
    // may fail; the answer is what counts.
    let _ = stream.write_all(&request).await;
    let (status, _) = response_head(&mut stream).await;
    assert_eq!(status, 431);
    assert_eq!(refusal_of(served.task.await.unwrap()), WebSocketRefusal::HeadTooLarge);
}

#[tokio::test]
async fn a_peer_that_sends_no_head_in_time_is_closed_without_an_answer() {
    let (mut stream, served) = serve(answering(), Arc::new(InstantClock));
    let result = served.task.await.unwrap();
    assert!(matches!(result, Err(TransportError::HandshakeTimedOut)), "{result:?}");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    assert!(answer.is_empty());
}

#[tokio::test]
async fn a_peer_that_leaves_inside_the_head_is_an_unexpected_end() {
    let (mut stream, served) = serve(answering(), Arc::new(StoppedClock));
    stream.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
    stream.shutdown().await.unwrap();
    match served.task.await.unwrap() {
        Err(TransportError::Io { source }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::UnexpectedEof);
        }
        other => panic!("expected an unexpected end, got {other:?}"),
    }
}

#[tokio::test]
async fn shutdown_during_the_handshake_ends_the_entry_quietly() {
    let (_stream, served) = serve(answering(), Arc::new(StoppedClock));
    served.shutdown.cancel();
    assert!(served.task.await.unwrap().is_ok());
}

/// Says hello, sends `bytes` and returns the close code that follows. The dispatcher
/// hears of the close.
async fn closes_with(bytes: &[u8]) -> u16 {
    let dispatcher = answering();
    let (mut client, served) = connected(Arc::clone(&dispatcher), Arc::new(StoppedClock)).await;
    client.hello().await;
    client.send_raw(bytes).await;
    let code = client.close_code().await;
    assert!(served.task.await.unwrap().is_ok());
    assert_eq!(dispatcher.closed_calls().len(), 1);
    code
}

#[tokio::test]
async fn a_binary_frame_closes_with_1003() {
    assert_eq!(closes_with(&client_frame(true, BINARY, b"{}", true)).await, 1003);
    assert_eq!(WebSocketCloseReason::Binary.code(), 1003);
}

#[tokio::test]
async fn a_fragment_or_a_continuation_closes_with_1003() {
    let start = client_frame(false, TEXT, br#"{"id":2,"#, true);
    assert_eq!(closes_with(&start).await, 1003);
    let continuation = client_frame(true, CONTINUATION, b"}", true);
    assert_eq!(closes_with(&continuation).await, 1003);
}

#[tokio::test]
async fn an_unmasked_frame_closes_with_1002() {
    let unmasked = client_frame(true, TEXT, &serde_json::to_vec(&list_frame(2)).unwrap(), false);
    assert_eq!(closes_with(&unmasked).await, 1002);
    let ping = client_frame(true, PING, b"x", false);
    assert_eq!(closes_with(&ping).await, 1002);
}

#[tokio::test]
async fn text_that_is_not_utf8_closes_with_1007() {
    assert_eq!(closes_with(&client_frame(true, TEXT, b"\xff\xfe", true)).await, 1007);
}

#[tokio::test]
async fn a_frame_head_with_reserved_bits_closes_with_1002() {
    let mut frame = client_frame(true, TEXT, b"{}", true);
    frame[0] |= 0x40;
    assert_eq!(closes_with(&frame).await, 1002);
}

/// A `conversations.list` request with id 2, padded with spaces to `len` bytes of JSON.
fn padded_list(len: usize) -> Vec<u8> {
    let mut json = serde_json::to_vec(&list_frame(2)).unwrap();
    json.resize(len, b' ');
    json
}

#[tokio::test]
async fn a_message_of_exactly_1_mib_is_served() {
    let (mut client, _served) = connected(answering(), Arc::new(StoppedClock)).await;
    client.hello().await;
    client.send_raw(&client_frame(true, TEXT, &padded_list(1024 * 1024), true)).await;
    assert_eq!(client.recv().await, ServerFrame::Item { id: id(2), item: json!({ "ok": true }) });
    assert_eq!(client.recv().await, ServerFrame::end(id(2)));
}

#[tokio::test]
async fn a_message_one_byte_over_1_mib_closes_with_1009_before_its_payload_is_read() {
    // Only the head goes out: the server refuses the length before it reads a payload.
    let full = client_frame(true, TEXT, &padded_list(1024 * 1024 + 1), true);
    assert_eq!(closes_with(&full[..2 + 8 + 4]).await, 1009);
}

#[tokio::test]
async fn a_ping_of_the_client_gets_its_pong() {
    let (mut client, _served) = connected(answering(), Arc::new(StoppedClock)).await;
    client.send_raw(&client_frame(true, PING, b"are you there", true)).await;
    assert_eq!(client.next().await, Some(Received::Pong(b"are you there".to_vec())));
}

#[tokio::test]
async fn a_close_of_the_client_is_echoed_and_ends_the_connection() {
    let dispatcher = answering();
    let (mut client, served) = connected(Arc::clone(&dispatcher), Arc::new(StoppedClock)).await;
    client.hello().await;
    client.send_raw(&client_frame(true, CLOSE, &1000_u16.to_be_bytes(), true)).await;
    assert_eq!(client.close_code().await, 1000);
    assert!(served.task.await.unwrap().is_ok());
    assert_eq!(dispatcher.closed_calls().len(), 1);
}

#[tokio::test]
async fn a_daemon_shutdown_closes_with_1001() {
    let (mut client, served) = connected(answering(), Arc::new(StoppedClock)).await;
    client.hello().await;
    served.shutdown.cancel();
    assert_eq!(client.close_code().await, 1001);
    assert!(served.task.await.unwrap().is_ok());
}

#[tokio::test]
async fn an_invalid_frame_with_an_id_is_answered_like_on_the_unix_socket() {
    let (mut client, _served) = connected(answering(), Arc::new(StoppedClock)).await;
    client.hello().await;
    client.send_raw(&client_frame(true, TEXT, br#"{"id":5,"method":"no.such"}"#, true)).await;
    match client.recv().await {
        ServerFrame::Error(frame) => {
            assert_eq!((frame.id, frame.error.code), (Some(id(5)), ErrorCode::Invalid));
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

/// A clock whose sleeps end only when a test fires them. A test waits until the code
/// under test sleeps, so the order of events is fixed and no real time passes.
#[derive(Debug, Default)]
struct ManualClock {
    sleeps: Mutex<Vec<(Duration, oneshot::Sender<()>)>>,
    started: Notify,
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_second(1_791_115_200).unwrap()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        let (fire, fired) = oneshot::channel();
        self.sleeps.lock().unwrap().push((duration, fire));
        self.started.notify_waiters();
        Box::pin(async move {
            if fired.await.is_err() {
                pending::<()>().await;
            }
        })
    }
}

impl ManualClock {
    /// Ends the oldest sleep of `duration` that still waits, after waiting until there
    /// is one.
    async fn fire(&self, duration: Duration) {
        loop {
            let started = self.started.notified();
            tokio::pin!(started);
            started.as_mut().enable();
            {
                let mut sleeps = self.sleeps.lock().unwrap();
                sleeps.retain(|(_, fire)| !fire.is_closed());
                if let Some(index) = sleeps.iter().position(|(length, _)| *length == duration) {
                    let (_, fire) = sleeps.remove(index);
                    let _ = fire.send(());
                    return;
                }
            }
            started.await;
        }
    }
}

#[tokio::test]
async fn the_server_pings_every_25_seconds_and_closes_after_two_missed_pongs() {
    let clock = Arc::new(ManualClock::default());
    let (mut client, served) = connected(answering(), Arc::clone(&clock) as Arc<dyn Clock>).await;
    client.hello().await;
    clock.fire(PING_INTERVAL).await;
    assert_eq!(client.next().await, Some(Received::Ping(1_u64.to_be_bytes().to_vec())));
    clock.fire(PING_INTERVAL).await;
    assert_eq!(client.next().await, Some(Received::Ping(2_u64.to_be_bytes().to_vec())));
    clock.fire(PING_INTERVAL).await;
    assert_eq!(client.close_code().await, WebSocketCloseReason::PingTimeout.code());
    assert!(served.task.await.unwrap().is_ok());
}

#[tokio::test]
async fn a_peer_that_answers_the_pings_stays_connected() {
    let clock = Arc::new(ManualClock::default());
    let (mut client, served) = connected(answering(), Arc::clone(&clock) as Arc<dyn Clock>).await;
    client.hello().await;
    for round in 0..5_u64 {
        clock.fire(PING_INTERVAL).await;
        let Some(Received::Ping(payload)) = client.next().await else {
            panic!("round {round}: expected a ping");
        };
        client.send_raw(&client_frame(true, PONG, &payload, true)).await;
        // The reader takes frames in order, so the answer shows the pong was taken.
        client.send(&list_frame(10 + round)).await;
        assert!(matches!(client.recv().await, ServerFrame::Item { .. }), "round {round}");
        assert_eq!(client.recv().await, ServerFrame::end(id(10 + round)));
    }
    assert!(!served.task.is_finished());
}

#[tokio::test]
async fn one_pong_answers_every_older_ping() {
    let clock = Arc::new(ManualClock::default());
    let (mut client, served) = connected(answering(), Arc::clone(&clock) as Arc<dyn Clock>).await;
    client.hello().await;
    clock.fire(PING_INTERVAL).await;
    assert!(matches!(client.next().await, Some(Received::Ping(_))));
    clock.fire(PING_INTERVAL).await;
    let Some(Received::Ping(newest)) = client.next().await else {
        panic!("expected the second ping");
    };
    client.send_raw(&client_frame(true, PONG, &newest, true)).await;
    client.send(&list_frame(3)).await;
    assert!(matches!(client.recv().await, ServerFrame::Item { .. }));
    assert_eq!(client.recv().await, ServerFrame::end(id(3)));
    clock.fire(PING_INTERVAL).await;
    assert_eq!(client.next().await, Some(Received::Ping(3_u64.to_be_bytes().to_vec())));
    assert!(!served.task.is_finished());
}

#[tokio::test]
async fn the_websocket_source_loses_no_frame_when_its_reads_are_cancelled() {
    let (mut client, server) = tokio::io::duplex(64);
    let (read, write) = tokio::io::split(server);
    let (mut source, _sink) =
        frames::start(read, write, Arc::new(StoppedClock), CancellationToken::new());
    let sending = tokio::spawn(async move {
        let mut bytes = Vec::new();
        for n in 0..40 {
            bytes.extend_from_slice(&text(&list_frame(n)));
        }
        // A few bytes at a time, so frames arrive while reads are cancelled.
        for chunk in bytes.chunks(7) {
            client.write_all(chunk).await.unwrap();
            tokio::task::yield_now().await;
        }
        client
    });
    let mut ids = Vec::new();
    let mut cancelled = 0;
    while ids.len() < 40 {
        tokio::select! {
            item = source.next_frame() => match item.unwrap().unwrap().unwrap() {
                ClientFrame::Request { id, .. } => ids.push(id.get()),
                other => panic!("unexpected frame {other:?}"),
            },
            () = tokio::task::yield_now() => cancelled += 1,
        }
    }
    assert_eq!(ids, (0..40).collect::<Vec<u64>>());
    assert!(cancelled > 0, "no read was cancelled");
    drop(sending.await.unwrap());
}

/// One end of either wire.
enum Wire {
    Unix(TestClient<DuplexStream>),
    Ws(Client),
}

impl Wire {
    async fn send(&mut self, frame: &ClientFrame) {
        match self {
            Wire::Unix(client) => client.send(frame).await,
            Wire::Ws(client) => client.send(frame).await,
        }
    }

    async fn recv(&mut self) -> ServerFrame {
        match self {
            Wire::Unix(client) => client.recv().await.unwrap(),
            Wire::Ws(client) => client.recv().await,
        }
    }

    /// Every server frame of a scripted session, in order: hello, a unary request, a
    /// subscription with two items and its cancel.
    async fn session(&mut self) -> Vec<ServerFrame> {
        let mut frames = Vec::new();
        for (frame, answers) in [
            (hello_frame(1, PROTOCOL_VERSION), 2),
            (list_frame(2), 2),
            (subscribe_frame(3), 2),
            (cancel_frame(3), 1),
        ] {
            self.send(&frame).await;
            for _ in 0..answers {
                frames.push(self.recv().await);
            }
        }
        frames
    }
}

/// Answers a unary request with `{"ok": true}`; a stream sends two items and waits for
/// its cancel.
fn scripted() -> Arc<FakeDispatcher> {
    FakeDispatcher::new(|request: Request| async move {
        if !request.method.is_stream() {
            return request.responder.item(&json!({ "ok": true })).await.map_err(internal);
        }
        for n in 1..=2 {
            request.responder.item(&json!({ "event": n })).await.map_err(internal)?;
        }
        pending::<()>().await;
        Ok(())
    })
}

#[tokio::test]
async fn the_websocket_and_the_unix_socket_give_the_same_frames() {
    let (unix_client, unix_server) = tokio::io::duplex(1 << 16);
    let params = ConnectionParams {
        conn_id: ConnId::new(7),
        peer: PeerCred::new(1000, Some(1)).into(),
        dispatcher: scripted(),
        clock: Arc::new(StoppedClock),
        shutdown: CancellationToken::new(),
    };
    tokio::spawn(connection::run(unix_server, params));
    let unix_frames = Wire::Unix(TestClient::new(unix_client)).session().await;
    let (client, _served) = connected(scripted(), Arc::new(StoppedClock)).await;
    let ws_frames = Wire::Ws(client).session().await;
    assert_eq!(ws_frames, unix_frames);
    assert_eq!(ws_frames.len(), 7);
    assert!(
        matches!(&ws_frames[6], ServerFrame::Error(frame) if frame.error.code == ErrorCode::Cancelled)
    );
}

type PipeSink = frames::WsSink<tokio::io::BufWriter<tokio::io::WriteHalf<DuplexStream>>>;

/// A WebSocket sink over a pipe and a queue that already holds `queued` end frames.
struct QueuedSink {
    client: Client,
    sink: PipeSink,
    _source: frames::WsSource,
    outbound: mpsc::Sender<EncodedFrame>,
    queue: mpsc::Receiver<EncodedFrame>,
    stopping: CancellationToken,
}

async fn queued_sink(queued: u64) -> QueuedSink {
    let (client, server) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(server);
    let stopping = CancellationToken::new();
    let (source, sink) = frames::start(read, write, Arc::new(StoppedClock), stopping.clone());
    let (outbound, queue) = mpsc::channel(16);
    for n in 0..queued {
        outbound.send(EncodedFrame::new(&ServerFrame::end(id(n))).unwrap()).await.unwrap();
    }
    QueuedSink {
        client: Client { stream: client },
        sink,
        _source: source,
        outbound,
        queue,
        stopping,
    }
}

#[tokio::test]
async fn the_sink_sends_every_queued_frame_and_then_a_normal_close() {
    let QueuedSink { mut client, sink, outbound, queue, .. } = queued_sink(3).await;
    drop(outbound);
    sink.drain(queue).await.unwrap();
    for n in 0..3 {
        assert_eq!(client.recv().await, ServerFrame::end(id(n)));
    }
    assert_eq!(client.next().await, Some(Received::Close(Some(1000), String::new())));
}

#[tokio::test]
async fn a_stopped_connection_drops_the_queued_frames_and_closes_with_1001() {
    let QueuedSink { mut client, sink, outbound, queue, stopping, .. } = queued_sink(8).await;
    stopping.cancel();
    // The queue never ends: the stop alone ends the writer.
    sink.drain(queue).await.unwrap();
    assert_eq!(
        client.next().await,
        Some(Received::Close(Some(1001), "the server closed the connection".to_owned()))
    );
    drop(outbound);
}
