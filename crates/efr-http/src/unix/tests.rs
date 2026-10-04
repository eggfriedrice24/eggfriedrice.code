use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixListener;

use super::UnixClient;
use crate::testing::{InstantClock, StoppedClock};
use crate::{HeaderValue, HttpError, HttpRequest, StatusCode};

const WHOIS: &str = "http://local-tailscaled.sock/localapi/v0/whois?addr=100.64.0.1:41641";
const WHOIS_REPLY: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 27\r\n\r\n{\"UserProfile\":{\"ID\":1234}}";

/// Accepts one connection, reads one request (head and `Content-Length` body), answers
/// with `reply` and returns the request as lowercase text.
async fn serve_once(listener: UnixListener, reply: &'static [u8]) -> String {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "the client closed before sending a whole request");
        request.extend_from_slice(&buffer[..read]);
        if let Some(head_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..head_end]).to_lowercase();
            let body_len = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map_or(0, |len| len.trim().parse::<usize>().unwrap());
            if request.len() >= head_end + 4 + body_len {
                break;
            }
        }
    }
    stream.write_all(reply).await.unwrap();
    String::from_utf8(request).unwrap().to_lowercase()
}

fn listen(dir: &Path) -> (std::path::PathBuf, UnixListener) {
    let socket = dir.join("tailscaled.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    (socket, listener)
}

#[tokio::test]
async fn get_sends_the_target_and_host_and_reads_the_reply() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, listener) = listen(dir.path());
    let server = tokio::spawn(serve_once(listener, WHOIS_REPLY));
    let client = UnixClient::new(&socket, Arc::new(StoppedClock));
    let request = HttpRequest::get(WHOIS)
        .unwrap()
        .header("tailscale-cap".parse().unwrap(), HeaderValue::from_static("1"));

    let response = client.send(&request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["UserProfile"]["ID"], 1234);

    let seen = server.await.unwrap();
    assert!(
        seen.starts_with("get /localapi/v0/whois?addr=100.64.0.1:41641 http/1.1\r\n"),
        "{seen}"
    );
    assert!(seen.contains("\r\nhost: local-tailscaled.sock\r\n"), "{seen}");
    assert!(seen.contains("\r\ntailscale-cap: 1\r\n"), "{seen}");
    assert_eq!(seen.matches("host:").count(), 1, "{seen}");
}

#[tokio::test]
async fn post_sends_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, listener) = listen(dir.path());
    let server = tokio::spawn(serve_once(listener, b"HTTP/1.1 204 No Content\r\n\r\n"));
    let client = UnixClient::new(&socket, Arc::new(StoppedClock));
    let request = HttpRequest::post("http://local-tailscaled.sock:8080/localapi/v0/ping")
        .unwrap()
        .json(&serde_json::json!({"ping": true}))
        .unwrap();
    let response = client.send(&request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response.bytes().await.unwrap().is_empty());
    let seen = server.await.unwrap();
    assert!(seen.contains("\r\nhost: local-tailscaled.sock:8080\r\n"), "{seen}");
    assert!(seen.ends_with("\r\n\r\n{\"ping\":true}"), "{seen}");
}

#[tokio::test]
async fn a_host_header_from_the_caller_wins() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, listener) = listen(dir.path());
    let server =
        tokio::spawn(serve_once(listener, b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"));
    let client = UnixClient::new(&socket, Arc::new(StoppedClock));
    let request = HttpRequest::get(WHOIS)
        .unwrap()
        .header(http::header::HOST, HeaderValue::from_static("custom.sock"));
    client.send(&request).await.unwrap().bytes().await.unwrap();
    let seen = server.await.unwrap();
    assert!(seen.contains("\r\nhost: custom.sock\r\n"), "{seen}");
    assert_eq!(seen.matches("host:").count(), 1, "{seen}");
}

#[tokio::test]
async fn a_missing_socket_is_a_connect_error() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("absent.sock");
    let client = UnixClient::new(&socket, Arc::new(StoppedClock));
    match client.send(&HttpRequest::get(WHOIS).unwrap()).await {
        Err(HttpError::UnixConnect { socket: failed, source }) => {
            assert_eq!(failed, socket);
            assert_eq!(source.kind(), ErrorKind::NotFound);
        }
        other => panic!("unexpected result {other:?}"),
    }
}

#[tokio::test]
async fn a_silent_server_times_out_on_the_clock() {
    let dir = tempfile::tempdir().unwrap();
    // Bound but never accepted: the connection succeeds and no answer ever comes.
    let (socket, _listener) = listen(dir.path());
    let clock = Arc::new(InstantClock::new());
    let client = UnixClient::new(&socket, clock.clone()).with_timeout(Duration::from_secs(3));
    match client.send(&HttpRequest::get(WHOIS).unwrap()).await {
        Err(HttpError::UnixTimedOut { socket: failed, after }) => {
            assert_eq!(failed, socket);
            assert_eq!(after, Duration::from_secs(3));
        }
        other => panic!("unexpected result {other:?}"),
    }
    assert_eq!(clock.sleeps(), [Duration::from_secs(3)]);
}

#[test]
fn defaults() {
    let client = UnixClient::new("/var/run/tailscale/tailscaled.sock", Arc::new(StoppedClock));
    assert_eq!(client.socket(), Path::new("/var/run/tailscale/tailscaled.sock"));
    assert_eq!(UnixClient::DEFAULT_TIMEOUT, Duration::from_secs(5));
}
