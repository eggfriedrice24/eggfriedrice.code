use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use fastwebsockets::{Frame, OpCode, Payload, Role, WebSocket as ServerSocket};
use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use super::{WsMessage, websocket_accept};
use crate::testing::{FixedRng, InstantClock};
use crate::{HeaderName, HeaderValue, HttpClient, HttpConfig, HttpError, HttpRequest, StatusCode};

/// The request head a fake server read: the request line and the headers, by lowercase
/// name.
#[derive(Debug, Clone)]
struct Head {
    line: String,
    headers: HashMap<String, String>,
}

/// How the fake server answers the handshake.
#[derive(Debug, Clone, Copy)]
enum Answer {
    /// `101` with the right accept value.
    Accept,
    /// `101` with an accept value for another key.
    WrongAccept,
    /// This status, with no upgrade.
    Status(u16),
}

async fn read_head(stream: &mut TcpStream) -> Head {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        bytes.push(byte[0]);
    }
    let text = String::from_utf8(bytes).unwrap();
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Head { line, headers }
}

/// A server on the loopback interface that answers one handshake with `answer` and then
/// runs `script` on the socket. The head it read comes back through the receiver.
async fn serve<F, Fut>(answer: Answer, script: F) -> (String, oneshot::Receiver<Head>)
where
    F: FnOnce(ServerSocket<TcpStream>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/socket", listener.local_addr().unwrap());
    let (seen, head) = oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_head(&mut stream).await;
        let key = request.headers.get("sec-websocket-key").cloned().unwrap_or_default();
        let _ = seen.send(request);
        let reply = match answer {
            Answer::Accept | Answer::WrongAccept => {
                let accept = match answer {
                    Answer::Accept => websocket_accept(&key),
                    _ => websocket_accept("another key"),
                };
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: {accept}\r\nx-test: yes\r\n\r\n"
                )
            }
            Answer::Status(status) => {
                format!("HTTP/1.1 {status} Nope\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            }
        };
        stream.write_all(reply.as_bytes()).await.unwrap();
        if matches!(answer, Answer::Accept) {
            let mut socket = ServerSocket::after_handshake(stream, Role::Server);
            socket.set_auto_pong(false);
            socket.set_auto_close(false);
            script(socket).await;
        }
    });
    (url, head)
}

fn client() -> HttpClient {
    HttpClient::new(&HttpConfig::default(), Arc::new(InstantClock::new()), Arc::new(FixedRng(7)))
        .unwrap()
}

fn text_frame(text: &str) -> Frame<'static> {
    Frame::text(Payload::Owned(text.as_bytes().to_vec()))
}

async fn next_text(socket: &mut ServerSocket<TcpStream>) -> String {
    loop {
        let frame = socket.read_frame().await.unwrap();
        if frame.opcode == OpCode::Text {
            return String::from_utf8(frame.payload.to_vec()).unwrap();
        }
    }
}

#[test]
fn the_accept_value_follows_the_example_of_the_rfc() {
    // RFC 6455, section 1.3.
    assert_eq!(websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
}

#[tokio::test]
async fn a_handshake_sends_the_request_headers_and_messages_go_both_ways() {
    let (url, head) = serve(Answer::Accept, |mut socket| async move {
        let text = next_text(&mut socket).await;
        socket.write_frame(text_frame(&format!("echo: {text}"))).await.unwrap();
        socket.write_frame(text_frame("second")).await.unwrap();
    })
    .await;
    let request = HttpRequest::get(&url)
        .unwrap()
        .header(HeaderName::from_static("x-trace"), HeaderValue::from_static("abc"));

    let mut socket = client().websocket(&request).await.unwrap();
    socket.send_text("hello".to_owned()).await.unwrap();

    assert_eq!(socket.next().await.unwrap().unwrap(), WsMessage::Text("echo: hello".to_owned()));
    assert_eq!(socket.next().await.unwrap().unwrap(), WsMessage::Text("second".to_owned()));
    assert_eq!(socket.response_headers().get("x-test").unwrap(), "yes");
    let head = head.await.unwrap();
    assert_eq!(head.line, "GET /socket HTTP/1.1");
    assert_eq!(head.headers["x-trace"], "abc");
    assert_eq!(head.headers["upgrade"], "websocket");
    assert_eq!(head.headers["sec-websocket-version"], "13");
    assert!(head.headers["user-agent"].starts_with("efr/"));
    assert_eq!(head.headers["sec-websocket-key"].len(), 24);
}

#[tokio::test]
async fn a_refused_upgrade_names_the_status() {
    let (url, _head) = serve(Answer::Status(426), |_| async {}).await;

    let error = client().websocket(&HttpRequest::get(&url).unwrap()).await.unwrap_err();

    assert!(
        matches!(error, HttpError::UpgradeRefused { status, .. } if status == StatusCode::UPGRADE_REQUIRED),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_answer_for_another_key_fails_the_handshake() {
    let (url, _head) = serve(Answer::WrongAccept, |_| async {}).await;

    let error = client().websocket(&HttpRequest::get(&url).unwrap()).await.unwrap_err();

    assert!(matches!(error, HttpError::Handshake { .. }), "{error:?}");
}

#[tokio::test]
async fn a_fragmented_message_arrives_whole_and_a_ping_gets_its_pong() {
    let (pong_tx, pong) = oneshot::channel();
    let (url, _head) = serve(Answer::Accept, |mut socket| async move {
        socket
            .write_frame(Frame::new(true, OpCode::Ping, None, b"p1".to_vec().into()))
            .await
            .unwrap();
        let frame = socket.read_frame().await.unwrap();
        let _ = pong_tx.send((frame.opcode == OpCode::Pong, frame.payload.to_vec()));
        socket
            .write_frame(Frame::new(false, OpCode::Text, None, b"one ".to_vec().into()))
            .await
            .unwrap();
        socket
            .write_frame(Frame::new(true, OpCode::Continuation, None, b"two".to_vec().into()))
            .await
            .unwrap();
    })
    .await;

    let mut socket = client().websocket(&HttpRequest::get(&url).unwrap()).await.unwrap();

    assert_eq!(socket.next().await.unwrap().unwrap(), WsMessage::Text("one two".to_owned()));
    assert_eq!(pong.await.unwrap(), (true, b"p1".to_vec()));
}

#[tokio::test]
async fn a_close_from_the_server_ends_the_messages() {
    let (url, _head) = serve(Answer::Accept, |mut socket| async move {
        socket.write_frame(text_frame("last")).await.unwrap();
        socket.write_frame(Frame::close(1000, b"bye")).await.unwrap();
        // The client echoes the close.
        let frame = socket.read_frame().await.unwrap();
        assert_eq!(frame.opcode, OpCode::Close);
    })
    .await;

    let mut socket = client().websocket(&HttpRequest::get(&url).unwrap()).await.unwrap();

    assert_eq!(socket.next().await.unwrap().unwrap(), WsMessage::Text("last".to_owned()));
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        WsMessage::Close { code: Some(1000), reason: "bye".to_owned() }
    );
    assert!(socket.next().await.is_none());
    assert!(socket.is_finished());
}

#[tokio::test]
async fn a_connection_that_drops_ends_with_an_error() {
    let (url, _head) = serve(Answer::Accept, |socket| async move {
        drop(socket);
    })
    .await;

    let mut socket = client().websocket(&HttpRequest::get(&url).unwrap()).await.unwrap();

    let error = socket.next().await.unwrap().unwrap_err();
    assert!(matches!(error, HttpError::WebSocket { .. }), "{error:?}");
    assert!(socket.next().await.is_none());
}

#[tokio::test]
async fn a_ping_ends_with_its_pong() {
    let (url, _head) = serve(Answer::Accept, |mut socket| async move {
        // An unrelated pong answers nothing; the pong with the ping's payload does.
        let ping = socket.read_frame().await.unwrap();
        assert_eq!(ping.opcode, OpCode::Ping);
        let payload = ping.payload.to_vec();
        socket.write_frame(Frame::pong(b"heartbeat".to_vec().into())).await.unwrap();
        socket.write_frame(text_frame("between")).await.unwrap();
        socket.write_frame(Frame::pong(payload.into())).await.unwrap();
        // Hold the connection open until the client goes.
        let _ = socket.read_frame().await;
    })
    .await;

    let mut socket = client().websocket(&HttpRequest::get(&url).unwrap()).await.unwrap();

    socket.ping().await.unwrap();
    assert_eq!(socket.next().await.unwrap().unwrap(), WsMessage::Text("between".to_owned()));
}

#[tokio::test]
async fn a_ping_fails_when_the_connection_ends_before_the_pong() {
    let (url, _head) = serve(Answer::Accept, |mut socket| async move {
        let ping = socket.read_frame().await.unwrap();
        assert_eq!(ping.opcode, OpCode::Ping);
        drop(socket);
    })
    .await;

    let socket = client().websocket(&HttpRequest::get(&url).unwrap()).await.unwrap();

    let error = socket.ping().await.unwrap_err();
    assert!(matches!(error, HttpError::WebSocketClosed { .. }), "{error:?}");
    // The reader has stopped, so a later ping fails at once.
    let error = socket.ping().await.unwrap_err();
    assert!(matches!(error, HttpError::WebSocketClosed { .. }), "{error:?}");
}
