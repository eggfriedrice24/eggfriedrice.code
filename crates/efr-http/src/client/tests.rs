use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use secrecy::SecretString;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{HttpClient, HttpConfig};
use crate::testing::{FixedRng, InstantClock};
use crate::{HttpError, HttpRequest, Record, Recorder, RetryPolicy, StatusCode};

fn client() -> (HttpClient, Arc<InstantClock>) {
    let clock = Arc::new(InstantClock::new());
    let client =
        HttpClient::new(&HttpConfig::default(), clock.clone(), Arc::new(FixedRng(0))).unwrap();
    (client, clock)
}

/// Waits of 1 s, 2 s, ... without jitter (the fixed generator draws 0, so each wait is
/// half of a base that starts at 2 s).
fn policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        initial_backoff: Duration::from_secs(2),
        max_backoff: Duration::from_secs(60),
        max_retry_after: Duration::from_secs(60),
    }
}

fn url(server: &MockServer, path: &str) -> String {
    format!("{}{path}", server.uri())
}

async fn requests_seen(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

#[test]
fn builds_with_the_default_config() {
    let (client, _clock) = client();
    let debug = format!("{client:?}");
    assert!(debug.contains("HttpClient"), "{debug}");
}

#[tokio::test]
async fn get_returns_status_headers_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("user-agent", concat!("efr/", env!("CARGO_PKG_VERSION"))))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-request-id", "req_1")
                .set_body_string("hello"),
        )
        .mount(&server)
        .await;
    let (client, _clock) = client();
    let response =
        client.send(&HttpRequest::get(&url(&server, "/v1/models")).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-request-id"], "req_1");
    assert_eq!(response.text().await.unwrap(), "hello");
}

#[tokio::test]
async fn post_sends_headers_and_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("originator", "efr"))
        .and(body_json(json!({"model": "gpt", "stream": true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "resp_1"})))
        .mount(&server)
        .await;
    let request = HttpRequest::post(&url(&server, "/v1/responses"))
        .unwrap()
        .bearer_auth(&SecretString::from("sk-test"))
        .unwrap()
        .header_text("originator".parse().unwrap(), "efr")
        .unwrap()
        .json(&json!({"model": "gpt", "stream": true}))
        .unwrap();
    let (client, _clock) = client();
    let body: serde_json::Value = client.send(&request).await.unwrap().json().await.unwrap();
    assert_eq!(body, json!({"id": "resp_1"}));
}

#[tokio::test]
async fn an_error_status_is_still_a_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    let (client, _clock) = client();
    let response =
        client.send(&HttpRequest::get(&url(&server, "/missing")).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn send_with_retry_retries_server_errors_on_the_clock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let (client, clock) = client();
    let request = HttpRequest::post(&url(&server, "/v1/responses")).unwrap();
    let response = client.send_with_retry(&request, &policy(4)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(requests_seen(&server).await, 3);
    assert_eq!(clock.sleeps(), [Duration::from_secs(1), Duration::from_secs(2)]);
}

#[tokio::test]
async fn send_with_retry_honours_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let (client, clock) = client();
    let request = HttpRequest::get(&url(&server, "/limited")).unwrap();
    let response = client.send_with_retry(&request, &policy(3)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(clock.sleeps(), [Duration::from_secs(7)]);
}

#[tokio::test]
async fn send_with_retry_returns_the_last_response_when_attempts_run_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(502)).mount(&server).await;
    let (client, clock) = client();
    let request = HttpRequest::get(&url(&server, "/down")).unwrap();
    let response = client.send_with_retry(&request, &policy(2)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(requests_seen(&server).await, 2);
    assert_eq!(clock.sleeps().len(), 1);
}

#[tokio::test]
async fn send_with_retry_does_not_retry_client_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let (client, clock) = client();
    let request = HttpRequest::get(&url(&server, "/private")).unwrap();
    let response = client.send_with_retry(&request, &policy(4)).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(requests_seen(&server).await, 1);
    assert_eq!(clock.sleeps(), []);
}

#[tokio::test]
async fn a_post_is_not_sent_again_after_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let (client, clock) = client();
    let request = HttpRequest::post(&url(&server, "/v1/responses")).unwrap();
    let response = client.send_with_retry(&request, &policy(4)).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(requests_seen(&server).await, 1, "the server may have acted on the first copy");
    assert_eq!(clock.sleeps(), []);
}

#[tokio::test]
async fn a_post_marked_idempotent_is_sent_again_after_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let (client, clock) = client();
    let request = HttpRequest::post(&url(&server, "/v1/responses")).unwrap().idempotent();
    let response = client.send_with_retry(&request, &policy(4)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(requests_seen(&server).await, 2);
    assert_eq!(clock.sleeps(), [Duration::from_secs(1)]);
}

#[tokio::test]
async fn a_post_is_sent_again_when_the_server_says_it_did_not_handle_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after-ms", "250"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let (client, clock) = client();
    let request = HttpRequest::post(&url(&server, "/v1/responses")).unwrap();
    let response = client.send_with_retry(&request, &policy(3)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(clock.sleeps(), [Duration::from_millis(250)]);
}

/// A TCP server on the loopback that handles each connection with `serve` and counts
/// the connections it accepted.
async fn raw_server<F, Fut>(serve: F) -> (String, Arc<AtomicUsize>)
where
    F: Fn(tokio::net::TcpStream) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(serve(stream));
        }
    });
    (address, accepted)
}

/// Reads one whole request whose body ends with `body`, then hangs up without an
/// answer: the server got the request, the client never learns what it did.
async fn read_then_hang_up(mut stream: tokio::net::TcpStream, body: &'static [u8]) {
    let mut seen = Vec::new();
    let mut buffer = [0; 1024];
    while !seen.ends_with(body) {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => seen.extend_from_slice(&buffer[..read]),
        }
    }
}

#[tokio::test]
async fn a_post_is_not_sent_again_when_the_connection_drops_after_the_body() {
    let (address, accepted) =
        raw_server(|stream| read_then_hang_up(stream, br#"{"input":"hello"}"#)).await;
    let (client, clock) = client();
    let request = HttpRequest::post(&format!("http://{address}/v1/responses"))
        .unwrap()
        .json(&json!({"input": "hello"}))
        .unwrap();
    let error = client.send_with_retry(&request, &policy(4)).await.unwrap_err();
    assert!(matches!(error, HttpError::Send { .. }), "{error:?}");
    assert!(!error.is_transient());
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(clock.sleeps(), []);
}

#[tokio::test]
async fn an_idempotent_request_is_sent_again_when_the_connection_drops() {
    let (address, accepted) =
        raw_server(|stream| read_then_hang_up(stream, br#"{"input":"hello"}"#)).await;
    let (client, clock) = client();
    let request = HttpRequest::post(&format!("http://{address}/v1/responses"))
        .unwrap()
        .json(&json!({"input": "hello"}))
        .unwrap()
        .idempotent();
    let error = client.send_with_retry(&request, &policy(2)).await.unwrap_err();
    assert!(matches!(error, HttpError::Send { .. }), "{error:?}");
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    assert_eq!(clock.sleeps().len(), 1);
}

#[tokio::test]
async fn a_failed_tls_handshake_is_a_connect_error_and_a_post_is_retried() {
    // Plain text where the client expects a TLS server hello: the handshake fails before
    // any byte of the request is sent.
    let (address, accepted) = raw_server(|mut stream| async move {
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
    })
    .await;
    let (client, clock) = client();
    let request = HttpRequest::post(&format!("https://{address}/v1/responses")).unwrap();
    let error = client.send_with_retry(&request, &policy(2)).await.unwrap_err();
    assert!(matches!(error, HttpError::Connect { .. }), "{error:?}");
    assert!(error.is_transient());
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    assert_eq!(clock.sleeps().len(), 1);
}

#[tokio::test]
async fn a_refused_connection_is_transient_and_retried_even_for_a_post() {
    // Bind and release a port so that nothing listens on it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let (client, clock) = client();
    let request = HttpRequest::post(&format!("http://{address}/v1/models?api_key=sk-1")).unwrap();
    let error = client.send_with_retry(&request, &policy(2)).await.unwrap_err();
    match &error {
        HttpError::Connect { url, .. } => {
            assert_eq!(url, &format!("http://{address}/v1/models?api_key=[REDACTED]"));
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert!(error.is_transient());
    assert_eq!(clock.sleeps().len(), 1);
    let mut text = format!("{error:?}");
    let mut source = std::error::Error::source(&error);
    while let Some(current) = source {
        text.push_str(&current.to_string());
        source = current.source();
    }
    assert!(!text.contains("sk-1"), "the unredacted URL leaked: {text}");
}

/// Keeps a one-line summary of each record.
#[derive(Debug, Default)]
struct Log(Mutex<Vec<String>>);

impl Log {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Recorder for Log {
    fn record(&self, record: &Record<'_>) {
        let line = match record {
            Record::Request { exchange, method, url, headers, body } => format!(
                "{exchange} request {method} {url} auth={:?} body={}",
                headers.get("authorization"),
                String::from_utf8_lossy(body)
            ),
            Record::Response { exchange, status, .. } => format!("{exchange} response {status}"),
            Record::BodyChunk { exchange, bytes } => {
                format!("{exchange} chunk {}", String::from_utf8_lossy(bytes))
            }
            Record::BodyEnd { exchange, end } => format!("{exchange} end {end:?}"),
            Record::Failed { exchange, .. } => format!("{exchange} failed"),
        };
        self.0.lock().unwrap().push(line);
    }
}

#[tokio::test]
async fn a_recorded_stream_reaches_the_recorder_redacted() {
    let server = MockServer::start().await;
    let sse = "event: response.output_text.delta\ndata: {\"delta\":\"hi\"}\n\n";
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;
    let log = Arc::new(Log::default());
    let (client, _clock) = client();
    let client = client.with_recorder(log.clone());
    let request = HttpRequest::post(&url(&server, "/v1/responses?key=k1"))
        .unwrap()
        .bearer_auth(&SecretString::from("sk-test"))
        .unwrap()
        .json(&json!({"input": "hello"}))
        .unwrap()
        .recorded();

    let events: Vec<_> = client.send(&request).await.unwrap().into_sse().collect().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].as_ref().unwrap().event, "response.output_text.delta");

    let base = server.uri();
    assert_eq!(
        log.lines(),
        [
            format!(
                "1 request POST {base}/v1/responses?key=[REDACTED] auth=Some(\"Bearer [REDACTED]\") body={{\"input\":\"hello\"}}"
            ),
            "1 response 200 OK".to_owned(),
            format!("1 chunk {sse}"),
            "1 end Complete".to_owned(),
        ]
    );
}

#[tokio::test]
async fn an_unrecorded_request_stays_out_of_the_recorder() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let log = Arc::new(Log::default());
    let (client, _clock) = client();
    let client = client.with_recorder(log.clone());
    let request = HttpRequest::post(&url(&server, "/oauth/token"))
        .unwrap()
        .form(&[("refresh_token", "rt-secret")]);
    client.send(&request).await.unwrap().bytes().await.unwrap();
    assert_eq!(log.lines(), Vec::<String>::new());
}

#[tokio::test]
async fn a_failed_recorded_request_is_recorded_as_failed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let log = Arc::new(Log::default());
    let (client, _clock) = client();
    let client = client.with_recorder(log.clone());
    let request = HttpRequest::get(&format!("http://{address}/")).unwrap().recorded();
    assert!(client.send(&request).await.is_err());
    let lines = log.lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1], "1 failed");
}

#[tokio::test]
async fn exchanges_are_numbered_per_client() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(204)).mount(&server).await;
    let log = Arc::new(Log::default());
    let (client, _clock) = client();
    let client = client.with_recorder(log.clone());
    let request = HttpRequest::get(&url(&server, "/ping")).unwrap().recorded();
    for _ in 0..2 {
        client.send(&request).await.unwrap().bytes().await.unwrap();
    }
    let responses: Vec<_> =
        log.lines().into_iter().filter(|line| line.contains("response")).collect();
    assert_eq!(responses, ["1 response 204 No Content", "2 response 204 No Content"]);
}

#[tokio::test]
async fn the_body_limit_applies_to_whole_reads() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0123456789"))
        .mount(&server)
        .await;
    let config = HttpConfig { max_body_bytes: 4, ..HttpConfig::default() };
    let client =
        HttpClient::new(&config, Arc::new(InstantClock::new()), Arc::new(FixedRng(0))).unwrap();
    let response = client.send(&HttpRequest::get(&url(&server, "/big")).unwrap()).await.unwrap();
    assert!(matches!(response.bytes().await, Err(HttpError::BodyTooLarge { limit: 4, .. })));
}
