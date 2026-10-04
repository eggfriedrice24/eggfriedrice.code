use std::error::Error as _;
use std::io;
use std::time::Duration;

use pretty_assertions::assert_eq;

use super::ProviderError;

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let cases = [
        (ProviderError::Unauthorized, "the provider rejected the credentials"),
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
fn rate_limits_name_the_delay_rounded_up() {
    let cases = [
        (None, "the provider is rate limiting requests"),
        (Some(Duration::from_secs(30)), "the provider is rate limiting requests; retry after 30s"),
        (
            Some(Duration::from_millis(1500)),
            "the provider is rate limiting requests; retry after 2s",
        ),
        (Some(Duration::from_millis(1)), "the provider is rate limiting requests; retry after 1s"),
        (Some(Duration::ZERO), "the provider is rate limiting requests; retry after 0s"),
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
