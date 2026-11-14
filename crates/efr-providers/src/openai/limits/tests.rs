use std::time::Duration;

use efr_http::{HeaderMap, HeaderName, HeaderValue};
use efr_provider::{LimitKind, LimitReached, LimitWindow};
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;

use super::{exhausted_bucket, plan_windows, plan_windows_message, reset_wait};

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
fn the_plan_headers_give_the_short_and_the_long_window() {
    let headers = headers(&[
        ("x-codex-primary-used-percent", "92.7"),
        ("x-codex-primary-window-minutes", "300"),
        ("x-codex-primary-reset-at", "1794666000"),
        ("x-codex-secondary-used-percent", "41.0"),
        ("x-codex-secondary-window-minutes", "10080"),
        ("x-codex-secondary-reset-at", "1795050000"),
        // NOTE: a family of one model has no row in efr.
        ("x-codex-other-primary-used-percent", "100.0"),
    ]);
    assert_eq!(
        plan_windows(&headers),
        vec![
            LimitWindow {
                minutes: Some(300),
                used_percent: 92,
                resets_at: Some(Timestamp::from_second(1_794_666_000).unwrap()),
            },
            LimitWindow {
                minutes: Some(10080),
                used_percent: 41,
                resets_at: Some(Timestamp::from_second(1_795_050_000).unwrap()),
            },
        ]
    );
}

#[rstest]
#[case::full("100.0", 100)]
#[case::over("130", 100)]
#[case::just_under("99.9", 99)]
#[case::below_zero("-3", 0)]
fn a_percent_is_rounded_down_into_0_to_100(#[case] text: &str, #[case] percent: u8) {
    let headers = headers(&[
        ("x-codex-primary-used-percent", text),
        ("x-codex-primary-window-minutes", "300"),
    ]);
    assert_eq!(plan_windows(&headers)[0].used_percent, percent);
}

#[test]
fn an_unused_window_with_no_length_and_no_reset_is_absent() {
    let headers =
        headers(&[("x-codex-primary-used-percent", "0"), ("x-codex-secondary-used-percent", "12")]);
    assert_eq!(
        plan_windows(&headers),
        vec![LimitWindow { minutes: None, used_percent: 12, resets_at: None }]
    );
}

#[rstest]
#[case::no_headers(&[])]
#[case::api_key_headers(&[("x-ratelimit-remaining-requests", "10")])]
#[case::no_percent(&[("x-codex-primary-window-minutes", "300")])]
#[case::bad_percent(&[("x-codex-primary-used-percent", "lots")])]
fn headers_without_a_plan_percent_give_no_window(#[case] pairs: &[(&str, &str)]) {
    assert_eq!(plan_windows(&headers(pairs)), Vec::new());
}

#[test]
fn the_websocket_message_gives_the_windows_of_the_default_family() {
    let message = json!({
        "type": "codex.rate_limits",
        "plan_type": "pro",
        "rate_limits": {
            "primary": {"used_percent": 31.5, "window_minutes": 300, "reset_at": 1_794_666_000},
            "secondary": {"used_percent": 63, "window_minutes": 10080},
        },
        "credits": {"has_credits": false, "unlimited": false},
    });
    assert_eq!(
        plan_windows_message(&message),
        Some(vec![
            LimitWindow {
                minutes: Some(300),
                used_percent: 31,
                resets_at: Some(Timestamp::from_second(1_794_666_000).unwrap()),
            },
            LimitWindow { minutes: Some(10080), used_percent: 63, resets_at: None },
        ])
    );
}

#[test]
fn the_websocket_message_of_another_family_gives_no_window() {
    let named = |name: &str| {
        json!({
            "type": "codex.rate_limits",
            "metered_limit_name": name,
            "rate_limits": {"primary": {"used_percent": 50, "window_minutes": 300}},
        })
    };
    assert_eq!(plan_windows_message(&named("codex_other")), Some(Vec::new()));
    assert_eq!(plan_windows_message(&named("Codex")).map(|windows| windows.len()), Some(1));
    assert_eq!(plan_windows_message(&json!({"type": "response.created"})), None);
}

#[test]
fn a_429_names_the_empty_bucket_of_the_key_with_its_reset() {
    let headers = headers(&[
        ("x-ratelimit-remaining-requests", "0"),
        ("x-ratelimit-reset-requests", "3m0s"),
        ("x-ratelimit-remaining-tokens", "1200"),
        ("x-ratelimit-reset-tokens", "6m0s"),
    ]);
    assert_eq!(
        exhausted_bucket(&headers, at(NOW), Some(Duration::from_secs(20))),
        Some(LimitReached {
            kind: LimitKind::Requests,
            resets_at: Some(at("2026-11-14T14:03:00Z"))
        })
    );
}

#[test]
fn of_two_empty_buckets_the_one_that_resets_last_counts() {
    let headers = headers(&[
        ("x-ratelimit-remaining-requests", "0"),
        ("x-ratelimit-reset-requests", "1s"),
        ("x-ratelimit-remaining-project-tokens", "0"),
        ("x-ratelimit-reset-project-tokens", "6m0s"),
    ]);
    assert_eq!(
        exhausted_bucket(&headers, at(NOW), None),
        Some(LimitReached { kind: LimitKind::Tokens, resets_at: Some(at("2026-11-14T14:06:00Z")) })
    );
}

#[test]
fn a_bucket_without_a_reset_takes_the_wait_of_the_answer() {
    let headers = headers(&[("x-ratelimit-remaining-tokens", "0")]);
    assert_eq!(
        exhausted_bucket(&headers, at(NOW), Some(Duration::from_secs(90))),
        Some(LimitReached { kind: LimitKind::Tokens, resets_at: Some(at("2026-11-14T14:01:30Z")) })
    );
}

#[test]
fn a_429_with_room_in_each_bucket_names_none() {
    let headers = headers(&[
        ("x-ratelimit-remaining-requests", "4"),
        ("x-ratelimit-remaining-tokens", "1200"),
    ]);
    assert_eq!(exhausted_bucket(&headers, at(NOW), Some(Duration::from_secs(1))), None);
    assert_eq!(exhausted_bucket(&HeaderMap::new(), at(NOW), None), None);
}

#[rstest]
#[case("1s", Some(1_000))]
#[case("6m0s", Some(360_000))]
#[case("20ms", Some(20))]
#[case("1h2m3.5s", Some(3_723_500))]
#[case("0.5s", Some(500))]
#[case("", None)]
#[case("soon", None)]
#[case("5", None)]
#[case("3d", None)]
fn reset_waits_read_the_samples_of_the_docs(#[case] text: &str, #[case] millis: Option<i64>) {
    assert_eq!(reset_wait(text).map(|wait| wait.as_millis() as i64), millis);
}
