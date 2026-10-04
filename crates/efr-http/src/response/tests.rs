use bytes::Bytes;
use futures::{StreamExt as _, stream};
use http::header::AUTHORIZATION;
use http::{HeaderMap, HeaderValue, StatusCode};
use pretty_assertions::assert_eq;
use serde::Deserialize;

use super::{ByteStream, HttpResponse};
use crate::HttpError;

const URL: &str = "https://api.example.com/v1/responses";

fn response(chunks: &[&'static [u8]], limit: usize) -> HttpResponse {
    let chunks: Vec<Result<Bytes, HttpError>> =
        chunks.iter().map(|chunk| Ok(Bytes::from_static(chunk))).collect();
    let body: ByteStream = Box::pin(stream::iter(chunks));
    HttpResponse::new(StatusCode::OK, HeaderMap::new(), URL.to_owned(), body, limit)
}

#[tokio::test]
async fn bytes_joins_the_chunks() {
    let body = response(&[b"abc", b"", b"def"], 64).bytes().await.unwrap();
    assert_eq!(body, Bytes::from_static(b"abcdef"));
}

#[tokio::test]
async fn bytes_accepts_a_body_of_exactly_the_limit() {
    assert_eq!(response(&[b"12", b"34"], 4).bytes().await.unwrap().len(), 4);
}

#[tokio::test]
async fn bytes_refuses_a_body_past_the_limit() {
    match response(&[b"12", b"345"], 4).bytes().await {
        Err(HttpError::BodyTooLarge { url, limit }) => {
            assert_eq!(url, URL);
            assert_eq!(limit, 4);
        }
        other => panic!("unexpected result {other:?}"),
    }
}

#[tokio::test]
async fn bytes_passes_a_body_error_through() {
    let chunks = vec![Ok(Bytes::from_static(b"a")), Err(HttpError::SseEventTooLarge { limit: 1 })];
    let body: ByteStream = Box::pin(stream::iter(chunks));
    let response = HttpResponse::new(StatusCode::OK, HeaderMap::new(), URL.to_owned(), body, 64);
    assert!(matches!(response.bytes().await, Err(HttpError::SseEventTooLarge { .. })));
}

#[tokio::test]
async fn text_replaces_invalid_utf8() {
    assert_eq!(response(&[b"ok \xFF"], 64).text().await.unwrap(), "ok \u{fffd}");
}

#[derive(Debug, Deserialize, PartialEq)]
struct Reply {
    id: String,
}

#[tokio::test]
async fn json_parses_the_body() {
    let reply: Reply = response(&[b"{\"id\":", b"\"resp_1\"}"], 64).json().await.unwrap();
    assert_eq!(reply, Reply { id: "resp_1".to_owned() });
}

#[tokio::test]
async fn json_reports_the_url_on_a_parse_error() {
    match response(&[b"<html>"], 64).json::<Reply>().await {
        Err(HttpError::DecodeJson { url, .. }) => assert_eq!(url, URL),
        other => panic!("unexpected result {other:?}"),
    }
}

#[tokio::test]
async fn into_sse_parses_events() {
    let mut events = response(&[b"data: a\n\n"], 64).into_sse();
    assert_eq!(events.next().await.unwrap().unwrap().data, "a");
    assert!(events.next().await.is_none());
}

#[test]
fn debug_redacts_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer sk-live"));
    let body: ByteStream = Box::pin(stream::empty());
    let response = HttpResponse::new(StatusCode::OK, headers, URL.to_owned(), body, 64);
    let debug = format!("{response:?}");
    assert!(!debug.contains("sk-live"), "{debug}");
    assert!(debug.contains("Bearer [REDACTED]"), "{debug}");
}
