use std::path::PathBuf;
use std::time::Duration;

use http::header::AUTHORIZATION;
use pretty_assertions::assert_eq;

use super::HttpError;

#[test]
fn a_timeout_is_not_transient_because_the_request_may_have_reached_the_server() {
    let url = || "https://api.openai.com/v1/responses".to_owned();
    assert!(!HttpError::Timeout { url: url() }.is_transient());
    assert!(!HttpError::BodyTooLarge { url: url(), limit: 1 }.is_transient());
    assert!(!HttpError::SseEventTooLarge { limit: 1 }.is_transient());
    assert!(
        !HttpError::UnixTimedOut { socket: PathBuf::from("/s"), after: Duration::from_secs(1) }
            .is_transient()
    );
}

#[test]
fn only_an_idempotent_request_is_retried_after_a_timeout() {
    let timeout = HttpError::Timeout { url: "https://api.openai.com/v1/responses".to_owned() };
    assert!(!timeout.is_retryable(false));
    assert!(timeout.is_retryable(true));
    for idempotent in [false, true] {
        let too_large = HttpError::BodyTooLarge { url: "https://x.test/".to_owned(), limit: 1 };
        assert!(!too_large.is_retryable(idempotent));
        assert!(!HttpError::SseEventTooLarge { limit: 1 }.is_retryable(idempotent));
    }
}

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let cases = [
        (
            HttpError::UnsupportedScheme { scheme: "ftp".to_owned() },
            r#"the URL scheme "ftp" is not http or https"#,
        ),
        (
            HttpError::InvalidHeaderValue { name: AUTHORIZATION },
            "the value of the authorization header is not a valid header value",
        ),
        (
            HttpError::Timeout { url: "https://x.test/a".to_owned() },
            "the request to https://x.test/a timed out",
        ),
        (
            HttpError::BodyTooLarge { url: "https://x.test/a".to_owned(), limit: 10 },
            "the response body from https://x.test/a is larger than 10 bytes",
        ),
        (HttpError::SseEventTooLarge { limit: 5 }, "a server-sent event is larger than 5 bytes"),
        (
            HttpError::UnixTimedOut {
                socket: PathBuf::from("/var/run/tailscale/tailscaled.sock"),
                after: Duration::from_secs(5),
            },
            "the socket /var/run/tailscale/tailscaled.sock did not answer within 5s",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}
