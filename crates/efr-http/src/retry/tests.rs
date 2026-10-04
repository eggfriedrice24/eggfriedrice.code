use std::cell::Cell;
use std::time::Duration;

use http::{HeaderMap, HeaderValue, StatusCode};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Decision, Outcome, RetryPolicy, Retryable, is_retryable_status, retry_after};
use efr_stdx::time::Clock as _;

use crate::testing::{FixedRng, InstantClock, start};

const NO_JITTER: FixedRng = FixedRng(0);
const SECOND: Duration = Duration::from_secs(1);

fn policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 4,
        initial_backoff: SECOND,
        max_backoff: Duration::from_secs(5),
        max_retry_after: Duration::from_secs(30),
    }
}

fn transient(retry_after: Option<Duration>) -> Outcome {
    Outcome::Transient { retry_after }
}

#[rstest]
#[case::final_outcome(1, Outcome::Final, Decision::Stop)]
#[case::first_failure(1, transient(None), Decision::RetryAfter(Duration::from_millis(500)))]
#[case::second_failure(2, transient(None), Decision::RetryAfter(SECOND))]
#[case::third_failure(3, transient(None), Decision::RetryAfter(Duration::from_secs(2)))]
#[case::attempts_used_up(4, transient(None), Decision::Stop)]
#[case::past_the_limit(9, transient(None), Decision::Stop)]
#[case::server_wait(
    1,
    transient(Some(Duration::from_secs(7))),
    Decision::RetryAfter(Duration::from_secs(7))
)]
#[case::server_wait_at_cap(
    1,
    transient(Some(Duration::from_secs(30))),
    Decision::RetryAfter(Duration::from_secs(30))
)]
#[case::server_wait_too_long(1, transient(Some(Duration::from_secs(31))), Decision::Stop)]
#[case::server_wait_but_no_attempts(4, transient(Some(SECOND)), Decision::Stop)]
fn decision_table(#[case] attempt: u32, #[case] outcome: Outcome, #[case] expected: Decision) {
    assert_eq!(policy().decide(attempt, outcome, &NO_JITTER), expected);
}

#[test]
fn none_never_retries() {
    assert_eq!(RetryPolicy::none().decide(1, transient(None), &NO_JITTER), Decision::Stop);
}

#[rstest]
#[case::first(1, Duration::from_millis(500))]
#[case::second(2, SECOND)]
#[case::third(3, Duration::from_secs(2))]
#[case::capped(4, Duration::from_millis(2500))]
#[case::far_beyond(40, Duration::from_millis(2500))]
#[case::zeroth_counts_as_first(0, Duration::from_millis(500))]
fn backoff_without_jitter_is_half_the_base(#[case] attempt: u32, #[case] expected: Duration) {
    assert_eq!(policy().backoff(attempt, &NO_JITTER), expected);
}

#[rstest]
#[case::first(1)]
#[case::second(2)]
#[case::capped(7)]
fn backoff_stays_between_half_and_the_whole_base(#[case] attempt: u32) {
    let base = SECOND.saturating_mul(1 << (attempt - 1)).min(Duration::from_secs(5));
    for draw in [0, 1, 999_999_999, u64::MAX / 3, u64::MAX] {
        let wait = policy().backoff(attempt, &FixedRng(draw));
        assert!(wait >= base / 2 && wait <= base, "attempt {attempt}, draw {draw}: {wait:?}");
    }
}

#[test]
fn backoff_survives_extreme_settings() {
    let policy = RetryPolicy {
        initial_backoff: Duration::MAX,
        max_backoff: Duration::MAX,
        ..RetryPolicy::default()
    };
    let wait = policy.backoff(3, &FixedRng(u64::MAX));
    assert!(wait >= Duration::MAX / 2);
    let zero = RetryPolicy { initial_backoff: Duration::ZERO, ..RetryPolicy::default() };
    assert_eq!(zero.backoff(1, &FixedRng(u64::MAX)), Duration::ZERO);
}

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    pairs
        .iter()
        .map(|(name, value)| (name.parse().unwrap(), HeaderValue::from_static(value)))
        .collect()
}

#[rstest]
#[case::none(&[], None)]
#[case::seconds(&[("retry-after", "12")], Some(Duration::from_secs(12)))]
#[case::seconds_with_spaces(&[("retry-after", " 3 ")], Some(Duration::from_secs(3)))]
#[case::millis(&[("retry-after-ms", "1500")], Some(Duration::from_millis(1500)))]
#[case::millis_win(&[("retry-after-ms", "250"), ("retry-after", "9")], Some(Duration::from_millis(250)))]
#[case::bad_millis_fall_back(&[("retry-after-ms", "soon"), ("retry-after", "9")], Some(Duration::from_secs(9)))]
#[case::future_date(&[("retry-after", "Sun, 04 Oct 2026 12:00:42 GMT")], Some(Duration::from_secs(42)))]
#[case::past_date(&[("retry-after", "Sun, 04 Oct 2026 11:59:00 GMT")], Some(Duration::ZERO))]
#[case::garbage(&[("retry-after", "later")], None)]
#[case::negative(&[("retry-after", "-5")], None)]
fn retry_after_header(
    #[case] pairs: &[(&'static str, &'static str)],
    #[case] expected: Option<Duration>,
) {
    assert_eq!(retry_after(&headers(pairs), start()), expected);
}

#[rstest]
#[case(408, true, true)]
#[case(429, true, true)]
#[case(503, true, true)]
#[case(500, false, true)]
#[case(502, false, true)]
#[case(504, false, true)]
#[case(200, false, false)]
#[case(400, false, false)]
#[case(401, false, false)]
#[case(404, false, false)]
#[case(409, false, false)]
#[case(501, false, false)]
fn retryable_statuses(#[case] status: u16, #[case] any: bool, #[case] idempotent: bool) {
    let status = StatusCode::from_u16(status).unwrap();
    assert_eq!(is_retryable_status(status, false), any, "not idempotent");
    assert_eq!(is_retryable_status(status, true), idempotent, "idempotent");
}

/// An attempt result for the loop tests, carrying its number and a fixed outcome.
#[derive(Debug, PartialEq)]
struct Attempt {
    number: u32,
    outcome: Outcome,
}

impl Retryable for Attempt {
    fn outcome(&self, _now: Timestamp) -> Outcome {
        self.outcome
    }
}

/// The loop tests never fail with an error, so this type is never constructed.
#[derive(Debug, PartialEq)]
struct NeverFails;

impl Retryable for NeverFails {
    fn outcome(&self, _now: Timestamp) -> Outcome {
        Outcome::Final
    }
}

#[tokio::test]
async fn run_retries_until_a_final_outcome() {
    let clock = InstantClock::new();
    let calls = Cell::new(0);
    let result: Result<Attempt, NeverFails> = policy()
        .run(&clock, &NO_JITTER, |number| {
            calls.set(calls.get() + 1);
            let outcome = if number < 3 { transient(None) } else { Outcome::Final };
            async move { Ok(Attempt { number, outcome }) }
        })
        .await;
    assert_eq!(result.unwrap(), Attempt { number: 3, outcome: Outcome::Final });
    assert_eq!(calls.get(), 3);
    assert_eq!(clock.sleeps(), [Duration::from_millis(500), SECOND]);
}

#[tokio::test]
async fn run_returns_the_last_result_when_attempts_run_out() {
    let clock = InstantClock::new();
    let result: Result<Attempt, NeverFails> = policy()
        .run(&clock, &NO_JITTER, |number| async move {
            Ok(Attempt { number, outcome: transient(None) })
        })
        .await;
    assert_eq!(result.unwrap().number, 4);
    assert_eq!(clock.sleeps().len(), 3);
}

#[tokio::test]
async fn run_honours_the_server_wait() {
    let clock = InstantClock::new();
    let _: Result<Attempt, NeverFails> = policy()
        .run(&clock, &NO_JITTER, |number| async move {
            let outcome =
                if number == 1 { transient(Some(Duration::from_secs(9))) } else { Outcome::Final };
            Ok(Attempt { number, outcome })
        })
        .await;
    assert_eq!(clock.sleeps(), [Duration::from_secs(9)]);
}

#[tokio::test]
async fn run_classifies_errors_too() {
    #[derive(Debug, PartialEq)]
    struct Flaky(u32);
    impl Retryable for Flaky {
        fn outcome(&self, _now: Timestamp) -> Outcome {
            transient(None)
        }
    }
    let clock = InstantClock::new();
    let result: Result<Attempt, Flaky> = policy()
        .run(&clock, &NO_JITTER, |number| async move {
            if number < 2 {
                Err(Flaky(number))
            } else {
                Ok(Attempt { number, outcome: Outcome::Final })
            }
        })
        .await;
    assert_eq!(result.unwrap().number, 2);
}

#[tokio::test]
async fn run_classifies_with_the_clock_time() {
    /// Transient while the clock still shows the start, final once a sleep moved it.
    #[derive(Debug)]
    struct UntilTimePasses(u32);
    impl Retryable for UntilTimePasses {
        fn outcome(&self, now: Timestamp) -> Outcome {
            if now == start() { transient(None) } else { Outcome::Final }
        }
    }
    let clock = InstantClock::new();
    let result: Result<UntilTimePasses, NeverFails> =
        policy().run(&clock, &NO_JITTER, |number| async move { Ok(UntilTimePasses(number)) }).await;
    assert_eq!(result.unwrap().0, 2);
    assert_eq!(clock.now(), start().checked_add(SignedDuration::from_millis(500)).unwrap());
}
