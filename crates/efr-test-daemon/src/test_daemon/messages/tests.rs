use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::{MessagesAnswer, MessagesServer};

const KEY: &str = "sk-ant-api03-test";

/// Sends `method` to `target` (a path and query) below the server, as plain HTTP/1.1
/// with the headers of the provider, and returns the head and the body of the response.
async fn send(server: &MessagesServer, method: &str, target: &str, body: &str) -> (String, String) {
    let base = server.base_url();
    let address = base.trim_start_matches("http://").trim_end_matches("/v1");
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "{method} {target} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {KEY}\r\n\
         anthropic-version: 2023-06-01\r\nanthropic-beta: thinking-binding-controls-2026-08-01\r\n\
         anthropic-workspace-id: wrkspc_test\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.to_owned(), body.to_owned())
}

/// The events of an SSE body, each `(type, data)`.
fn events(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .filter(|event| !event.is_empty())
        .map(|event| {
            let (kind, data) = event.split_once('\n').unwrap();
            let data: Value = serde_json::from_str(data.strip_prefix("data: ").unwrap()).unwrap();
            (kind.strip_prefix("event: ").unwrap().to_owned(), data)
        })
        .collect()
}

#[test]
fn a_text_answer_streams_one_text_block_and_ends_the_turn() {
    let answer = MessagesAnswer::text("Hi there.");

    assert_eq!(answer.status, 200);
    let events = events(&answer.body);
    let kinds: Vec<&str> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "message_start",
            "ping",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    for (kind, data) in &events {
        assert_eq!(data["type"], json!(kind));
    }
    assert_eq!(events[3].1["delta"], json!({"type": "text_delta", "text": "Hi there."}));
    assert_eq!(events[5].1["delta"]["stop_reason"], json!("end_turn"));
    assert_eq!(events[0].1["message"]["usage"]["input_tokens"], json!(10));
}

#[test]
fn a_tool_use_answer_streams_its_input_in_two_parts() {
    let input = json!({"command": "ls -la", "timeout_seconds": 600});
    let answer = MessagesAnswer::tool_use("toolu_01", "shell", &input);

    let events = events(&answer.body);
    assert_eq!(events[1].1["content_block"]["type"], json!("tool_use"));
    assert_eq!(events[1].1["content_block"]["id"], json!("toolu_01"));
    assert_eq!(events[1].1["content_block"]["name"], json!("shell"));
    let parts: String = events[2..4]
        .iter()
        .map(|(_, data)| data["delta"]["partial_json"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(serde_json::from_str::<Value>(&parts).unwrap(), input);
    assert_eq!(events[5].1["delta"]["stop_reason"], json!("tool_use"));
}

#[tokio::test]
async fn model_calls_take_the_queued_answers_in_order_and_are_kept() {
    let server = MessagesServer::start().await;
    server.push(MessagesAnswer::error(529, "overloaded_error", "Overloaded").with_retry_after(3));
    server.push(MessagesAnswer::text("Hi."));

    let first = send(&server, "POST", "/v1/messages", r#"{"n":1}"#).await;
    let second = send(&server, "POST", "/v1/messages", r#"{"n":2}"#).await;
    let third = send(&server, "POST", "/v1/messages", "not json").await;

    assert!(first.0.starts_with("HTTP/1.1 529"), "{first:?}");
    assert!(first.0.contains("retry-after: 3"), "{first:?}");
    assert!(first.0.contains("request-id: req_test_1"), "{first:?}");
    let error: Value = serde_json::from_str(&first.1).unwrap();
    assert_eq!(error["error"]["type"], json!("overloaded_error"));
    assert!(second.0.starts_with("HTTP/1.1 200"), "{second:?}");
    assert!(second.0.contains("content-type: text/event-stream"), "{second:?}");
    assert_eq!(second.1, MessagesAnswer::text("Hi.").body);
    assert!(third.0.starts_with("HTTP/1.1 599"), "{third:?}");
    let received = server.received();
    let bodies: Vec<&Value> = received.iter().map(|request| &request.body).collect();
    assert_eq!(bodies, [&json!({"n": 1}), &json!({"n": 2}), &Value::Null]);
    assert_eq!(received[0].authorization.as_deref(), Some(format!("Bearer {KEY}").as_str()));
    assert_eq!(received[0].api_key, None);
    assert_eq!(received[0].version.as_deref(), Some("2023-06-01"));
    assert_eq!(received[0].beta.as_deref(), Some("thinking-binding-controls-2026-08-01"));
    assert_eq!(received[0].workspace_id.as_deref(), Some("wrkspc_test"));
    assert!(!format!("{:?}", received[0]).contains(KEY), "Debug hides the key");
    assert_eq!(server.remaining(), 0);
}

#[tokio::test]
async fn the_model_list_comes_in_pages() {
    let server = MessagesServer::start().await;
    let ids = ["claude-fable-5-1", "claude-opus-5-5", "claude-haiku-5-5"];
    server.set_models(ids.iter().map(|id| MessagesServer::model(id)).collect());

    let (_, first) = send(&server, "GET", "/v1/models?limit=2", "").await;
    let (_, second) = send(&server, "GET", "/v1/models?limit=2&after_id=claude-opus-5-5", "").await;
    let (_, default) = send(&server, "GET", "/v1/models", "").await;

    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["data"].as_array().unwrap().len(), 2);
    assert_eq!(first["has_more"], json!(true));
    assert_eq!(first["first_id"], json!("claude-fable-5-1"));
    assert_eq!(first["last_id"], json!("claude-opus-5-5"));
    let second: Value = serde_json::from_str(&second).unwrap();
    assert_eq!(second["data"][0]["id"], json!("claude-haiku-5-5"));
    assert_eq!(second["data"][0]["lifecycle"], json!("active"));
    assert_eq!(second["has_more"], json!(false));
    let default: Value = serde_json::from_str(&default).unwrap();
    assert_eq!(default["data"].as_array().unwrap().len(), 3, "20 models a page by default");
    let requests = server.models_requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].limit.as_deref(), Some("2"));
    assert_eq!(requests[1].after_id.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(requests[0].version.as_deref(), Some("2023-06-01"));
    assert_eq!(requests[0].workspace_id.as_deref(), Some("wrkspc_test"));
    assert!(!format!("{:?}", requests[0]).contains(KEY), "Debug hides the key");
}

#[tokio::test]
async fn the_model_list_can_be_refused_and_is_missing_until_it_is_set() {
    let server = MessagesServer::start().await;

    let (missing, _) = send(&server, "GET", "/v1/models?limit=1", "").await;
    server.refuse_models(MessagesAnswer::error(401, "authentication_error", "invalid x-api-key"));
    let (refused, body) = send(&server, "GET", "/v1/models?limit=1", "").await;
    server.set_models(vec![MessagesServer::model("claude-opus-5-5")]);
    let (listed, _) = send(&server, "GET", "/v1/models?limit=1", "").await;

    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");
    let error: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(error["error"]["message"], json!("invalid x-api-key"));
    assert!(listed.starts_with("HTTP/1.1 200"), "a new list ends the refusal: {listed}");
}
