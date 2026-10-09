use std::time::Duration;

use efr_http::{Outcome, Retryable as _, StatusCode};
use efr_provider::{ProviderError, SecretString};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::{Again, Failure, answer, event, without_key};
use crate::testing::start;

const MODEL: &str = "claude-opus-5-5";

/// An error body of the API with `kind` and `message`.
fn body(kind: &str, message: &str) -> String {
    json!({
        "type": "error",
        "error": {"type": kind, "message": message},
        "request_id": "req_011CVyqRZ",
    })
    .to_string()
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap()
}

/// The class of an error, as a short name that a table can hold.
fn class(error: &ProviderError) -> String {
    match error {
        ProviderError::ContextOverflow { status, .. } => format!("overflow {status:?}"),
        ProviderError::Unauthorized { .. } => "unauthorized".to_owned(),
        ProviderError::UnknownModel { model } => format!("unknown model {model}"),
        ProviderError::RateLimited { retry_after } => format!("rate limited {retry_after:?}"),
        ProviderError::Overloaded { status, .. } => format!("overloaded {status:?}"),
        ProviderError::Api { status, code, .. } => format!("api {status:?} {code:?}"),
        other => format!("{other:?}"),
    }
}

#[rstest]
#[case::prompt_too_long(
    400,
    body("invalid_request_error", "prompt is too long: 213462 tokens > 200000 maximum"),
    "overflow Some(400)",
    Again::Never
)]
#[case::usage_limits(
    400,
    body(
        "invalid_request_error",
        "You have reached your specified API usage limits. You will regain access on 2026-11-01 at 00:00 UTC."
    ),
    "api Some(400) Some(\"invalid_request_error\")",
    Again::Never
)]
#[case::workspace_missing(
    400,
    body(
        "invalid_request_error",
        "anthropic-workspace-id is required when authenticating with an identity-linked API key"
    ),
    "api Some(400) Some(\"invalid_request_error\")",
    Again::Never
)]
#[case::key_refused(
    401,
    body("authentication_error", "invalid x-api-key"),
    "unauthorized",
    Again::Never
)]
#[case::billing(
    402,
    body("billing_error", "Your credit balance is too low."),
    "api Some(402) Some(\"billing_error\")",
    Again::Never
)]
#[case::permission(
    403,
    body("permission_error", "This key cannot use the model."),
    "api Some(403) Some(\"permission_error\")",
    Again::Never
)]
#[case::model(
    404,
    body("not_found_error", "model: claude-opus-5-5"),
    "unknown model claude-opus-5-5",
    Again::Never
)]
#[case::wrong_path(404, "<html>Not Found</html>".to_owned(), "api Some(404) None", Again::Never)]
#[case::too_large(
    413,
    body("request_too_large", "Request exceeds the maximum size"),
    "api Some(413) Some(\"request_too_large\")",
    Again::Never
)]
#[case::too_large_from_the_edge(413, "Payload Too Large".to_owned(), "api Some(413) None", Again::Never)]
#[case::rate_limit(
    429,
    body("rate_limit_error", "Number of request tokens has exceeded your per-minute rate limit"),
    "rate limited None",
    Again::After(None)
)]
#[case::server(
    500,
    body("api_error", "Internal server error"),
    "api Some(500) Some(\"api_error\")",
    Again::After(None)
)]
#[case::proxy(502, "Bad Gateway".to_owned(), "api Some(502) None", Again::After(None))]
#[case::unavailable(503, String::new(), "api Some(503) None", Again::After(None))]
#[case::timeout(
    504,
    body("timeout_error", "Request timed out"),
    "api Some(504) Some(\"timeout_error\")",
    Again::After(None)
)]
#[case::overloaded(
    529,
    body("overloaded_error", "Overloaded"),
    "overloaded Some(529)",
    Again::After(None)
)]
#[case::unknown_status(
    409,
    body("conflict_error", "Conflict"),
    "api Some(409) Some(\"conflict_error\")",
    Again::Never
)]
fn each_answer_has_its_class(
    #[case] code: u16,
    #[case] text: String,
    #[case] expected: &str,
    #[case] again: Again,
) {
    let failure = answer(status(code), None, &text, Some(MODEL));

    assert_eq!(class(&failure.error), expected, "{failure:?}");
    assert_eq!(failure.again, again);
}

#[test]
fn a_spend_cap_is_never_sent_again() {
    let text = json!({
        "type": "error",
        "error": {
            "type": "rate_limit_error",
            "message": "This request would exceed your organization's spend limit.",
            "details": {"error_code": "enforced_spend_limit_reached"},
        },
    })
    .to_string();

    let failure = answer(status(429), None, &text, Some(MODEL));

    assert_eq!(failure.again, Again::Never);
    match failure.error {
        ProviderError::Api { status: Some(429), code, message } => {
            assert_eq!(code.as_deref(), Some("enforced_spend_limit_reached"));
            assert_eq!(message, "This request would exceed your organization's spend limit.");
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[test]
fn a_wait_from_the_headers_goes_to_the_rate_limit_and_to_the_policy() {
    let wait = Some(Duration::from_secs(30));
    let failure = answer(status(429), wait, &body("rate_limit_error", "slow down"), Some(MODEL));

    assert!(
        matches!(failure.error, ProviderError::RateLimited { retry_after } if retry_after == wait)
    );
    assert_eq!(failure.outcome(start()), Outcome::Transient { retry_after: wait });
}

#[test]
fn a_401_keeps_the_servers_message_and_a_bare_one_has_none() {
    let failure =
        answer(status(401), None, &body("authentication_error", "invalid x-api-key"), None);
    assert!(
        matches!(&failure.error, ProviderError::Unauthorized { message: Some(text) } if text == "invalid x-api-key"),
        "{failure:?}"
    );

    let bare = answer(status(401), None, "", None);
    assert!(matches!(bare.error, ProviderError::Unauthorized { message: None }), "{bare:?}");
}

#[test]
fn a_404_without_a_model_is_an_api_error() {
    let failure = answer(status(404), None, &body("not_found_error", "Not found"), None);

    assert_eq!(class(&failure.error), "api Some(404) Some(\"not_found_error\")");
}

#[test]
fn the_message_comes_from_the_body_and_is_clipped() {
    let long = "x".repeat(5000);
    let failure = answer(status(400), None, &body("invalid_request_error", &long), None);
    let ProviderError::Api { message, .. } = failure.error else { panic!("{failure:?}") };
    assert_eq!(message.chars().count(), 1000);

    let failure = answer(status(529), None, "", None);
    let ProviderError::Overloaded { message, .. } = failure.error else { panic!("{failure:?}") };
    assert_eq!(message, "no message", "529 has no reason phrase");

    let failure = answer(status(502), None, "  ", None);
    let ProviderError::Api { message, .. } = failure.error else { panic!("{failure:?}") };
    assert_eq!(message, "Bad Gateway");
}

#[test]
fn a_failure_never_sent_again_is_final_for_the_policy() {
    let failure =
        Failure { error: ProviderError::Unauthorized { message: None }, again: Again::Never };
    assert_eq!(failure.outcome(start()), Outcome::Final);
}

#[rstest]
#[case::overloaded("overloaded_error", "Overloaded", "overloaded None")]
#[case::server("api_error", "Internal server error", "api None Some(\"api_error\")")]
#[case::rate_limit("rate_limit_error", "slow down", "rate limited None")]
#[case::prompt_too_long(
    "invalid_request_error",
    "prompt is too long: 5 tokens > 4 maximum",
    "overflow None"
)]
#[case::key("authentication_error", "invalid x-api-key", "unauthorized")]
#[case::model("not_found_error", "model: claude-x", "api None Some(\"not_found_error\")")]
#[case::unknown_type("shiny_new_error", "Something new", "api None Some(\"shiny_new_error\")")]
fn an_error_event_has_the_class_of_its_type(
    #[case] kind: &str,
    #[case] message: &str,
    #[case] expected: &str,
) {
    let data: Value = serde_json::from_str(&body(kind, message)).unwrap();

    assert_eq!(class(&event(&data)), expected);
}

#[test]
fn an_error_event_without_an_error_is_an_api_error() {
    let error = event(&json!({"type": "error"}));

    match error {
        ProviderError::Api { status: None, code: None, message } => {
            assert_eq!(message, "no message")
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[rstest]
#[case::a_quoted_key(
    "bad key sk-ant-1 here, sk-ant-1",
    "sk-ant-1",
    "bad key <the key> here, <the key>"
)]
#[case::no_key_in_the_body("bad key", "sk-ant-1", "bad key")]
#[case::an_empty_key("bad key", "", "bad key")]
fn a_body_never_shows_the_key(#[case] body: &str, #[case] key: &str, #[case] shown: &str) {
    assert_eq!(without_key(body, &SecretString::from(key)), shown);
}
