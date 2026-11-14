use std::time::Duration;

use efr_http::{HeaderMap, HeaderName, HeaderValue};
use efr_provider::{LimitKind, LimitReached};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::exhausted_bucket;

const NOW: &str = "2026-11-14T14:00:00Z";

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

#[test]
fn a_429_names_the_empty_bucket_with_the_time_of_its_header() {
    let headers = headers(&[
        ("anthropic-ratelimit-requests-remaining", "12"),
        ("anthropic-ratelimit-requests-reset", "2026-11-14T14:00:05Z"),
        ("anthropic-ratelimit-input-tokens-remaining", "0"),
        ("anthropic-ratelimit-input-tokens-reset", "2026-11-14T14:03:00Z"),
        ("retry-after", "20"),
    ]);
    assert_eq!(
        exhausted_bucket(&headers, at(NOW), Some(Duration::from_secs(20))),
        Some(LimitReached {
            kind: LimitKind::InputTokens,
            resets_at: Some(at("2026-11-14T14:03:00Z")),
        })
    );
}

#[test]
fn of_two_empty_buckets_the_one_that_resets_last_counts() {
    let headers = headers(&[
        ("anthropic-ratelimit-requests-remaining", "0"),
        ("anthropic-ratelimit-requests-reset", "2026-11-14T14:00:30Z"),
        ("anthropic-ratelimit-output-tokens-remaining", "0"),
        ("anthropic-ratelimit-output-tokens-reset", "2026-11-14T14:01:00Z"),
    ]);
    assert_eq!(
        exhausted_bucket(&headers, at(NOW), None).map(|reached| reached.kind),
        Some(LimitKind::OutputTokens)
    );
}

#[test]
fn a_bucket_with_a_bad_reset_takes_the_wait_of_the_answer() {
    let headers = headers(&[
        ("anthropic-ratelimit-tokens-remaining", "0"),
        ("anthropic-ratelimit-tokens-reset", "soon"),
    ]);
    assert_eq!(
        exhausted_bucket(&headers, at(NOW), Some(Duration::from_secs(60))),
        Some(LimitReached { kind: LimitKind::Tokens, resets_at: Some(at("2026-11-14T14:01:00Z")) })
    );
}

#[test]
fn a_429_with_room_in_each_bucket_names_none() {
    let headers = headers(&[("anthropic-ratelimit-requests-remaining", "3")]);
    assert_eq!(exhausted_bucket(&headers, at(NOW), Some(Duration::from_secs(1))), None);
    assert_eq!(exhausted_bucket(&HeaderMap::new(), at(NOW), None), None);
}
