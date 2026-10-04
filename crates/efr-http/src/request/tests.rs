use std::time::Duration;

use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderName, HeaderValue, Method};
use pretty_assertions::assert_eq;
use secrecy::SecretString;
use serde_json::json;

use super::HttpRequest;
use crate::HttpError;

const URL: &str = "https://auth.openai.com/oauth/token";

#[test]
fn new_parses_the_url() {
    let request = HttpRequest::get("https://api.openai.com/v1/models?limit=2").unwrap();
    assert_eq!(request.method(), Method::GET);
    assert_eq!(request.url().path(), "/v1/models");
    assert_eq!(request.url().query(), Some("limit=2"));
    assert!(request.body_bytes().is_empty());
    assert!(!request.is_recorded());
    assert_eq!(request.deadline(), None);
}

#[test]
fn a_post_is_idempotent_only_when_marked() {
    assert!(!HttpRequest::post(URL).unwrap().is_idempotent());
    assert!(HttpRequest::post(URL).unwrap().idempotent().is_idempotent());
    assert!(!HttpRequest::new(Method::PATCH, URL).unwrap().is_idempotent());
    for method in [Method::GET, Method::HEAD, Method::OPTIONS, Method::PUT, Method::DELETE] {
        assert!(HttpRequest::new(method.clone(), URL).unwrap().is_idempotent(), "{method}");
    }
}

#[test]
fn new_rejects_a_relative_url_without_echoing_it() {
    let error = HttpRequest::get("/v1/models?api_key=sk-1").unwrap_err();
    assert!(matches!(error, HttpError::InvalidUrl { .. }), "{error:?}");
    assert!(!format!("{error:?} {error}").contains("sk-1"));
}

#[test]
fn new_rejects_other_schemes() {
    match HttpRequest::post("ftp://example.com/file") {
        Err(HttpError::UnsupportedScheme { scheme }) => assert_eq!(scheme, "ftp"),
        other => panic!("unexpected result {other:?}"),
    }
}

#[test]
fn json_sets_the_body_and_content_type() {
    let request = HttpRequest::post(URL).unwrap().json(&json!({"stream": true})).unwrap();
    assert_eq!(request.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(request.body_bytes().as_ref(), br#"{"stream":true}"#);
}

#[test]
fn form_encodes_the_pairs() {
    let request = HttpRequest::post(URL)
        .unwrap()
        .form(&[("grant_type", "refresh_token"), ("refresh_token", "a b&c=d")]);
    assert_eq!(request.headers()[CONTENT_TYPE], "application/x-www-form-urlencoded");
    assert_eq!(
        request.body_bytes().as_ref(),
        b"grant_type=refresh_token&refresh_token=a+b%26c%3Dd"
    );
}

#[test]
fn header_replaces_an_earlier_value() {
    let originator = HeaderName::from_static("originator");
    let request = HttpRequest::get(URL)
        .unwrap()
        .header(originator.clone(), HeaderValue::from_static("one"))
        .header(originator.clone(), HeaderValue::from_static("two"));
    let values: Vec<_> = request.headers().get_all(&originator).iter().collect();
    assert_eq!(values, ["two"]);
}

#[test]
fn header_text_rejects_a_newline_without_echoing_it() {
    let name = HeaderName::from_static("chatgpt-account-id");
    match HttpRequest::get(URL).unwrap().header_text(name.clone(), "acct\r\nX-Evil: 1") {
        Err(error @ HttpError::InvalidHeaderValue { .. }) => {
            assert!(!error.to_string().contains("acct"), "{error}");
        }
        other => panic!("unexpected result {other:?}"),
    }
    let request = HttpRequest::get(URL).unwrap().header_text(name.clone(), "acct-1").unwrap();
    assert_eq!(request.headers()[&name], "acct-1");
}

#[test]
fn bearer_auth_marks_the_header_sensitive() {
    let token = SecretString::from("sk-live-123");
    let request = HttpRequest::get(URL).unwrap().bearer_auth(&token).unwrap();
    let value = &request.headers()[AUTHORIZATION];
    assert_eq!(value, "Bearer sk-live-123");
    assert!(value.is_sensitive());
}

#[test]
fn bearer_auth_rejects_a_token_with_a_newline() {
    let token = SecretString::from("sk\nlive");
    let result = HttpRequest::get(URL).unwrap().bearer_auth(&token);
    assert!(matches!(result, Err(HttpError::InvalidHeaderValue { .. })));
}

#[test]
fn debug_shows_neither_the_body_nor_secrets() {
    let request = HttpRequest::post("https://x.test/cb?code=abc123")
        .unwrap()
        .bearer_auth(&SecretString::from("sk-live-123"))
        .unwrap()
        .form(&[("refresh_token", "rt-secret")])
        .timeout(Duration::from_secs(30))
        .recorded();
    let debug = format!("{request:?}");
    for secret in ["sk-live-123", "rt-secret", "abc123"] {
        assert!(!debug.contains(secret), "{secret} leaked into {debug}");
    }
    assert!(debug.contains("body_len: 23"), "{debug}");
    assert!(debug.contains("recorded: true"), "{debug}");
}
