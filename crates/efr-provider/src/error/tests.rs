use std::error::Error as _;
use std::io;
use std::time::Duration;

use pretty_assertions::assert_eq;

use super::ProviderError;

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let cases = [
        (ProviderError::Unauthorized { message: None }, "the provider rejected the credentials"),
        (
            ProviderError::Unauthorized { message: Some("invalid x-api-key".to_owned()) },
            "the provider rejected the credentials: invalid x-api-key",
        ),
        (ProviderError::NotLoggedIn, "no credentials are stored for the provider"),
        (
            ProviderError::UnknownModel { model: "gpt-0".to_owned() },
            r#"the provider does not serve the model "gpt-0""#,
        ),
        (
            ProviderError::InvalidStream { problem: "a tool call started twice" },
            "the provider's event stream is malformed: a tool call started twice",
        ),
        (
            ProviderError::Incomplete,
            "the provider's event stream ended before the response was done",
        ),
        (
            ProviderError::InvalidProviderId { id: "Open AI".to_owned() },
            r#""Open AI" is not a valid provider id"#,
        ),
        (
            ProviderError::Api {
                status: Some(400),
                code: Some("invalid_request_error".to_owned()),
                message: "Unsupported parameter: 'temperature'".to_owned(),
            },
            "the provider answered with an error: Unsupported parameter: 'temperature'",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn rate_limits_name_the_delay_rounded_up_in_its_two_largest_units() {
    let cases = [
        (None, "the provider is rate limiting requests"),
        (Some(Duration::from_secs(30)), "the provider is rate limiting requests; retry after 30s"),
        (
            Some(Duration::from_millis(1500)),
            "the provider is rate limiting requests; retry after 2s",
        ),
        (Some(Duration::from_millis(1)), "the provider is rate limiting requests; retry after 1s"),
        (Some(Duration::ZERO), "the provider is rate limiting requests; retry after 0s"),
        (
            Some(Duration::from_secs(303)),
            "the provider is rate limiting requests; retry after 5m 3s",
        ),
        (
            Some(Duration::from_secs(3 * 3_600)),
            "the provider is rate limiting requests; retry after 3h 0m",
        ),
        (
            Some(Duration::from_secs(2 * 86_400 + 4 * 3_600 + 59)),
            "the provider is rate limiting requests; retry after 2d 4h",
        ),
    ];
    for (retry_after, expected) in cases {
        assert_eq!(ProviderError::RateLimited { retry_after }.to_string(), expected);
    }
}

#[test]
fn wrapped_failures_keep_their_source_out_of_the_message() {
    let error = ProviderError::Transport { source: Box::new(io::Error::other("connection reset")) };
    assert_eq!(error.to_string(), "the request to the provider failed in transit");
    assert_eq!(error.source().unwrap().to_string(), "connection reset");

    let error = ProviderError::Token { source: Box::new(io::Error::other("refresh failed")) };
    assert_eq!(error.to_string(), "the token source could not produce an access token");
    assert_eq!(error.source().unwrap().to_string(), "refresh failed");
}

#[test]
fn decode_failures_chain_the_parser_error() {
    let source = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    let error = ProviderError::Decode { source };
    assert_eq!(error.to_string(), "the provider sent a response that could not be parsed");
    assert!(error.source().unwrap().to_string().contains("EOF"));
}

#[test]
fn an_api_error_that_says_the_request_does_not_fit_is_a_context_overflow() {
    let code = ProviderError::api(
        None,
        Some("context_length_exceeded".to_owned()),
        "Your input exceeds the context window of this model.".to_owned(),
    );
    let status = ProviderError::api(Some(413), None, "Payload Too Large".to_owned());
    let message = ProviderError::api(
        Some(400),
        Some("invalid_request_error".to_owned()),
        "prompt is too long: 210000 tokens > 200000 maximum".to_owned(),
    );

    for error in [&code, &status, &message] {
        assert!(error.is_context_overflow(), "{error:?}");
        assert_eq!(error.to_string(), "the request is larger than the model's context window");
    }
    let ProviderError::ContextOverflow { status, code, message } = code else { unreachable!() };
    assert_eq!(status, None);
    assert_eq!(code.as_deref(), Some("context_length_exceeded"));
    assert_eq!(message, "Your input exceeds the context window of this model.");
}

#[test]
fn any_other_api_error_stays_an_api_error() {
    let error = ProviderError::api(
        Some(400),
        Some("invalid_request_error".to_owned()),
        "the prompt is too long for this tool".to_owned(),
    );

    assert!(!error.is_context_overflow());
    assert!(matches!(error, ProviderError::Api { status: Some(400), .. }), "{error:?}");
}
