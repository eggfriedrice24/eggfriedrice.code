use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use efr_http::{HeaderName, HeaderValue, HttpClient, HttpConfig, HttpRequest, Recorder};
use efr_stdx::time::Clock as _;
use efr_test_support::{TestClock, TestRng};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{Item, StreamRecorder, StreamRecording, Writer, model_url, run_dir};

/// The secret values of the headers of the test: no file may hold any of them.
const SECRETS: [&str; 5] =
    ["sk-test-bearer-0042", "sk-ant-test-0042", "sid=cookie-0042", "acct-0042", "set-0042"];

/// A recorded answer of the Responses API, as a provider fixture holds one.
const SSE: &str = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\"}\n\n";

/// Every file of `dir`, by name, with its text.
fn files(dir: &Path) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&path).unwrap())
        })
        .collect();
    files.sort();
    files
}

/// The mode bits of `path`.
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// `request` with the four secret request headers.
fn with_secrets(request: HttpRequest) -> HttpRequest {
    let header =
        |name, value| (HeaderName::from_static(name), HeaderValue::from_str(value).unwrap());
    [
        header("authorization", "Bearer sk-test-bearer-0042"),
        header("x-api-key", "sk-ant-test-0042"),
        header("cookie", "sid=cookie-0042"),
        header("chatgpt-account-id", "acct-0042"),
    ]
    .into_iter()
    .fold(request, |request, (name, value)| request.header(name, value))
    .recorded()
}

/// Sends `request` through `client` and reads its whole answer.
async fn read_all(client: &HttpClient, request: &HttpRequest) {
    let response = client.send(request).await.unwrap();
    let mut body = response.into_stream();
    while let Some(chunk) = body.next().await {
        chunk.unwrap();
    }
}

/// Ends `recording` and returns its directory once the writer wrote everything.
fn finish(recording: StreamRecording, client: HttpClient) -> std::path::PathBuf {
    let StreamRecording { recorder, dir, writer } = recording;
    drop(client);
    drop(recorder);
    writer.join().unwrap();
    dir
}

#[test]
fn only_the_urls_of_model_calls_are_taken_and_without_query_or_user() {
    assert_eq!(
        model_url("https://chatgpt.com/backend-api/codex/responses?trace=1").as_deref(),
        Some("https://chatgpt.com/backend-api/codex/responses")
    );
    assert_eq!(
        model_url("https://user:pass@api.anthropic.com/v1/messages#x").as_deref(),
        Some("https://api.anthropic.com/v1/messages")
    );
    assert_eq!(
        model_url("wss://api.openai.com/v1/responses").as_deref(),
        Some("wss://api.openai.com/v1/responses")
    );
    for other in [
        "https://auth.openai.com/oauth/token",
        "https://api.anthropic.com/v1/models",
        "https://api.openai.com/v1/responses/compact",
        "not a url",
    ] {
        assert_eq!(model_url(other), None, "{other}");
    }
}

#[tokio::test]
async fn a_model_call_is_written_without_a_header_and_other_calls_are_not_written() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "set-0042")
                .set_body_raw(SSE, "text/event-stream"),
        )
        .mount(&server)
        .await;
    let state = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let recording = StreamRecorder::start(state.path(), clock.shared()).unwrap();
    let client = HttpClient::new(&HttpConfig::default(), clock.shared(), Arc::new(TestRng::new(1)))
        .unwrap()
        .with_recorder(Arc::clone(&recording.recorder) as Arc<dyn Recorder>);

    let body = json!({"model": "gpt-5.5", "input": "hello", "stream": true});
    let call = HttpRequest::post(&format!("{}/v1/responses?key=sk-test-bearer-0042", server.uri()))
        .unwrap()
        .json(&body)
        .unwrap();
    read_all(&client, &with_secrets(call)).await;
    let token = HttpRequest::post(&format!("{}/oauth/token", server.uri()))
        .unwrap()
        .form(&[("refresh_token", "sk-test-bearer-0042")]);
    read_all(&client, &with_secrets(token)).await;

    let dir = finish(recording, client);
    let found = files(&dir);
    let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["1.request.json", "1.response.sse", "index.jsonl"]);
    for (name, text) in &found {
        for secret in SECRETS {
            assert!(!text.contains(secret), "{name} holds {secret}");
        }
        assert_eq!(mode(&dir.join(name)), 0o600, "{name}");
    }
    assert_eq!(mode(&dir), 0o700);
    let request: Value = serde_json::from_str(&found[0].1).unwrap();
    let url = format!("{}/v1/responses", server.uri());
    assert_eq!(request, json!({"transport": "http", "method": "POST", "url": url, "body": body}));
    assert_eq!(found[1].1, SSE, "the answer is a provider fixture as it is");
    let index: Value = serde_json::from_str(found[2].1.trim_end()).unwrap();
    assert_eq!(index["n"], 1);
    assert_eq!(index["transport"], "http");
    assert_eq!(index["status"], 200);
    assert_eq!(index["end"], "complete");
    assert_eq!(index["at"], clock.now().to_string());
    let parent = dir.parent().unwrap();
    assert_eq!(parent, state.path().join("streams"));
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with(&format!("-{}", std::process::id())), "{name}");
}

#[test]
fn the_messages_of_a_socket_become_one_call_for_each_request() {
    let state = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let dir = run_dir(state.path(), clock.now(), 77).unwrap();
    let url = model_url("wss://api.openai.com/v1/responses?trace=1").unwrap();
    let (items, queue) = std::sync::mpsc::sync_channel(16);
    let say = |sent, text: &str| {
        let item = Item::Socket {
            exchange: 4,
            at: clock.now(),
            url: url.clone(),
            sent,
            text: text.to_owned(),
        };
        items.send(item).unwrap();
    };
    say(true, r#"{"type":"response.create","model":"gpt-5.5"}"#);
    say(false, r#"{"type":"response.created"}"#);
    say(false, r#"{"type":"response.completed"}"#);
    say(true, r#"{"type":"response.create","previous_response_id":"resp_1"}"#);
    say(false, r#"{"type":"response.output_text.delta","delta":"a"}"#);
    drop(items);
    Writer::new(dir.clone()).run(&queue);

    let found = files(&dir);
    let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["1.request.json", "1.response.sse", "2.request.json", "2.response.sse", "index.jsonl"]
    );
    let first: Value = serde_json::from_str(&found[0].1).unwrap();
    assert_eq!(
        first,
        json!({
            "transport": "websocket",
            "method": "GET",
            "url": "wss://api.openai.com/v1/responses",
            "body": {"type": "response.create", "model": "gpt-5.5"}
        })
    );
    assert_eq!(
        found[1].1,
        "event: response.created\ndata: {\"type\":\"response.created\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\"}\n\n"
    );
    let ends: Vec<Value> =
        found[4].1.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(ends[0]["end"], "complete");
    assert_eq!(ends[0]["status"], 101);
    assert_eq!(ends[1]["n"], 2);
    assert_eq!(ends[1]["end"], "dropped", "the socket ended before the answer");
    assert_eq!(dir.file_name().unwrap().to_string_lossy(), "20261004T120000Z-77");
}

/// The lines of `index.jsonl` and the answer files of the writer after `items`.
fn written(items: Vec<Item>) -> (Vec<Value>, Vec<(String, String)>) {
    let state = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let dir = run_dir(state.path(), clock.now(), 77).unwrap();
    let (sender, queue) = std::sync::mpsc::sync_channel(64);
    for item in items {
        sender.send(item).unwrap();
    }
    drop(sender);
    Writer::new(dir.clone()).run(&queue);
    let found = files(&dir);
    let index = found
        .iter()
        .find(|(name, _)| name == "index.jsonl")
        .map(|(_, text)| text.lines().map(|line| serde_json::from_str(line).unwrap()).collect())
        .unwrap_or_default();
    let answers = found.into_iter().filter(|(name, _)| name.ends_with(".response.sse")).collect();
    (index, answers)
}

fn request(exchange: u64) -> Item {
    Item::Request {
        exchange,
        at: TestClock::new().now(),
        method: "POST".to_owned(),
        url: "https://api.anthropic.com/v1/messages".to_owned(),
        body: br#"{"model":"m"}"#.to_vec(),
    }
}

fn chunk(exchange: u64, text: &str) -> Item {
    Item::Chunk { exchange, bytes: text.as_bytes().to_vec() }
}

#[test]
fn an_answer_that_the_provider_stopped_reading_after_its_last_event_is_complete() {
    let long = "x".repeat(200);
    let (index, _) = written(vec![
        request(1),
        Item::Response { exchange: 1, status: 200 },
        chunk(1, &format!("event: content_block_delta\ndata: {{\"t\":\"{long}\"}}\n\nevent: mess")),
        chunk(1, "age_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
        Item::End { exchange: 1, end: "dropped" },
        request(2),
        Item::Response { exchange: 2, status: 200 },
        chunk(2, "event: content_block_delta\ndata: {}\n\n"),
        Item::End { exchange: 2, end: "dropped" },
        request(3),
        Item::Response { exchange: 3, status: 200 },
        chunk(3, "event: response.failed\r\ndata: {}\r\n\r\n"),
        Item::End { exchange: 3, end: "dropped" },
    ]);
    let ends: Vec<&str> = index.iter().map(|line| line["end"].as_str().unwrap()).collect();
    assert_eq!(ends, ["complete", "dropped", "failed"]);
}

#[test]
fn the_body_of_a_refused_credential_is_not_written() {
    // OpenAI's 401 quotes the first and the last characters of the key it refused.
    let refused = r#"{"error":{"message":"Incorrect API key provided: sk-fake*******enai."}}"#;
    let (index, answers) = written(vec![
        request(1),
        Item::Response { exchange: 1, status: 401 },
        chunk(1, refused),
        Item::End { exchange: 1, end: "complete" },
        request(2),
        Item::Response { exchange: 2, status: 400 },
        chunk(2, r#"{"error":{"message":"Web search is not enabled."}}"#),
        Item::End { exchange: 2, end: "complete" },
    ]);
    assert_eq!(answers[0].1, "", "the 401 body");
    assert_eq!(answers[1].1, r#"{"error":{"message":"Web search is not enabled."}}"#);
    assert_eq!(index[0]["status"], 401);
    assert_eq!(index[0]["end"], "complete");
}
