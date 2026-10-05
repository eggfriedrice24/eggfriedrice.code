use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::{ResponsesAnswer, ResponsesServer};

/// Posts `body` to `path` below the server, as plain HTTP/1.1, and returns the status
/// line and the response body.
async fn post(server: &ResponsesServer, path: &str, body: &str) -> (String, String) {
    let base = server.base_url();
    let address = base.trim_start_matches("http://").trim_end_matches("/v1");
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer sk-test\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

#[test]
fn an_sse_text_is_a_200_answer_unless_its_first_line_names_a_status() {
    let plain = ResponsesAnswer::from_sse("event: x\ndata: {}\n\n");
    let refused = ResponsesAnswer::from_sse(": status 401\n\n{\"error\":{}}\n");

    assert_eq!(plain, ResponsesAnswer::new(200, "event: x\ndata: {}\n\n"));
    assert_eq!(refused, ResponsesAnswer::new(401, "{\"error\":{}}\n"));
    assert_eq!(ResponsesAnswer::from_sse(": status x\n\n").status, 200, "not a number");
}

#[test]
fn a_text_answer_streams_one_message_from_created_to_completed() {
    let answer = ResponsesAnswer::text("Hi there.");

    assert_eq!(answer.status, 200);
    let events: Vec<(&str, serde_json::Value)> = answer
        .body
        .split("\n\n")
        .filter(|event| !event.is_empty())
        .map(|event| {
            let (kind, data) = event.split_once('\n').unwrap();
            let data: serde_json::Value =
                serde_json::from_str(data.strip_prefix("data: ").unwrap()).unwrap();
            (kind.strip_prefix("event: ").unwrap(), data)
        })
        .collect();
    let kinds: Vec<&str> = events.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(
        kinds,
        [
            "response.created",
            "response.output_item.added",
            "response.content_part.added",
            "response.output_text.delta",
            "response.output_text.done",
            "response.content_part.done",
            "response.output_item.done",
            "response.completed",
        ]
    );
    for (index, (kind, data)) in events.iter().enumerate() {
        assert_eq!(data["type"], *kind);
        assert_eq!(data["sequence_number"], index);
    }
    assert_eq!(events[3].1["delta"], "Hi there.");
    assert_eq!(events[7].1["response"]["output"][0]["content"][0]["text"], "Hi there.");
}

#[test]
fn a_tool_call_answer_sends_the_whole_call_in_one_item() {
    let arguments = serde_json::json!({ "command": "true", "timeout_seconds": 600 });
    let answer = ResponsesAnswer::tool_call("call_1", "shell", &arguments);

    assert_eq!(answer.status, 200);
    let events: Vec<serde_json::Value> = answer
        .body
        .split("\n\n")
        .filter(|event| !event.is_empty())
        .map(|event| {
            let (_, data) = event.split_once('\n').unwrap();
            serde_json::from_str(data.strip_prefix("data: ").unwrap()).unwrap()
        })
        .collect();
    let kinds: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["response.created", "response.output_item.done", "response.completed"]);
    let item = &events[1]["item"];
    assert_eq!(item["type"], "function_call");
    assert_eq!(item["call_id"], "call_1");
    assert_eq!(item["name"], "shell");
    let sent: serde_json::Value =
        serde_json::from_str(item["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(sent, arguments);
    assert_eq!(events[2]["response"]["output"][0], *item);
}

#[tokio::test]
async fn requests_take_the_queued_answers_in_order_and_are_kept() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::new(401, "{\"error\":{}}"));
    server.push(ResponsesAnswer::new(200, "data: done\n\n"));

    let first = post(&server, "/v1/responses", r#"{"n":1}"#).await;
    let second = post(&server, "/v1/responses", r#"{"n":2}"#).await;
    let third = post(&server, "/v1/responses", "not json").await;
    let elsewhere = post(&server, "/v1/other", "{}").await;

    assert_eq!(first, ("HTTP/1.1 401 Unauthorized".to_owned(), "{\"error\":{}}".to_owned()));
    assert_eq!(second, ("HTTP/1.1 200 OK".to_owned(), "data: done\n\n".to_owned()));
    assert!(third.0.starts_with("HTTP/1.1 599"), "{third:?}");
    assert!(elsewhere.0.starts_with("HTTP/1.1 404"), "{elsewhere:?}");
    let received = server.received();
    let bodies: Vec<&serde_json::Value> = received.iter().map(|request| &request.body).collect();
    assert_eq!(bodies, [&json!({ "n": 1 }), &json!({ "n": 2 }), &serde_json::Value::Null]);
    assert_eq!(received[0].authorization.as_deref(), Some("Bearer sk-test"));
    assert!(!format!("{:?}", received[0]).contains("sk-test"), "Debug hides the key");
    assert_eq!(server.remaining(), 0);
}
