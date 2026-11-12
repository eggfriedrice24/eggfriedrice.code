use std::time::Duration;

use efr_test_support::TestClock;

use super::{HTTP_WARM_GAP, Warmups};

#[test]
fn an_http_warm_up_fetches_at_most_once_a_minute_for_each_provider() {
    let clock = TestClock::new();
    let warmups = Warmups::new(clock.shared());

    assert!(warmups.http_due("openai-api"));
    assert!(!warmups.http_due("openai-api"));
    assert!(warmups.http_due("anthropic-api"));
    clock.advance(HTTP_WARM_GAP - Duration::from_secs(1));
    assert!(!warmups.http_due("openai-api"));
    clock.advance(Duration::from_secs(1));
    assert!(warmups.http_due("openai-api"));
}

#[test]
fn an_http_warm_up_does_not_fetch_within_a_minute_of_a_prompt_of_its_provider() {
    let clock = TestClock::new();
    let warmups = Warmups::new(clock.shared());

    warmups.used("anthropic-api");
    clock.advance(Duration::from_secs(59));
    assert!(!warmups.http_due("anthropic-api"));
    assert!(warmups.http_due("openai-api"));
    clock.advance(Duration::from_secs(1));
    assert!(warmups.http_due("anthropic-api"));
}
