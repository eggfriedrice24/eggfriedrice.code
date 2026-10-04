use std::future::pending;
use std::time::Duration;

use efr_stdx::StdxError;
use efr_stdx::time::Clock;
use futures::FutureExt as _;
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::TestClock;

const SECOND: Duration = Duration::from_secs(1);

fn at(seconds_after_start: i64) -> Timestamp {
    TestClock::START.checked_add(SignedDuration::from_secs(seconds_after_start)).unwrap()
}

#[test]
fn a_new_clock_reads_the_fixed_start() {
    assert_eq!(TestClock::new().now().to_string(), "2026-10-04T12:00:00Z");
    assert_eq!(TestClock::default().now(), TestClock::START);
}

#[test]
fn a_clock_can_start_anywhere() {
    let start = Timestamp::UNIX_EPOCH;
    assert_eq!(TestClock::starting_at(start).now(), start);
}

#[test]
fn advance_moves_now_and_clones_see_it() {
    let clock = TestClock::new();
    let shared = clock.shared();
    let clone = clock.clone();
    clock.advance(90 * SECOND);
    assert_eq!(shared.now(), at(90));
    assert_eq!(clone.now(), at(90));
}

#[test]
fn advance_stops_at_the_last_timestamp() {
    let clock = TestClock::new();
    clock.advance(Duration::MAX);
    assert_eq!(clock.now(), Timestamp::MAX);
}

#[test]
fn a_sleep_of_zero_is_done_at_once() {
    let clock = TestClock::new();
    assert_eq!(clock.sleep(Duration::ZERO).now_or_never(), Some(()));
    assert_eq!(clock.pending_sleeps(), 0);
}

#[test]
fn a_sleep_ends_when_the_clock_reaches_its_deadline() {
    let clock = TestClock::new();
    let mut sleep = clock.sleep(30 * SECOND);
    assert_eq!((&mut sleep).now_or_never(), None);
    clock.advance(29 * SECOND);
    assert_eq!((&mut sleep).now_or_never(), None);
    clock.advance(SECOND);
    assert_eq!(sleep.now_or_never(), Some(()));
    assert_eq!(clock.pending_sleeps(), 0);
}

#[test]
fn a_sleep_that_is_never_polled_still_ends() {
    let clock = TestClock::new();
    let sleep = clock.sleep(SECOND);
    clock.advance(SECOND);
    assert_eq!(clock.pending_sleeps(), 0);
    assert_eq!(sleep.now_or_never(), Some(()));
}

#[test]
fn a_dropped_sleep_stops_counting() {
    let clock = TestClock::new();
    let sleep = clock.sleep(SECOND);
    assert_eq!(clock.pending_sleeps(), 1);
    drop(sleep);
    assert_eq!(clock.pending_sleeps(), 0);
    assert_eq!(clock.next_deadline(), None);
}

#[test]
fn advance_to_next_deadline_ends_the_earliest_sleep_first() {
    let clock = TestClock::new();
    let mut late = clock.sleep(10 * SECOND);
    let early = clock.sleep(5 * SECOND);
    assert_eq!(clock.next_deadline(), Some(at(5)));

    assert_eq!(clock.advance_to_next_deadline(), Some(5 * SECOND));
    assert_eq!(early.now_or_never(), Some(()));
    assert_eq!((&mut late).now_or_never(), None);

    assert_eq!(clock.advance_to_next_deadline(), Some(5 * SECOND));
    assert_eq!(late.now_or_never(), Some(()));

    assert_eq!(clock.advance_to_next_deadline(), None);
    assert_eq!(clock.now(), at(10));
}

#[test]
fn a_sleep_past_the_last_timestamp_never_ends() {
    let clock = TestClock::new();
    let mut sleep = clock.sleep(Duration::MAX);
    clock.advance(Duration::MAX);
    assert_eq!((&mut sleep).now_or_never(), None);
    assert_eq!(clock.pending_sleeps(), 1);
    assert_eq!(clock.next_deadline(), None);
    assert_eq!(clock.advance_to_next_deadline(), None);
}

#[test]
fn a_step_back_makes_a_sleep_last_longer() {
    let clock = TestClock::new();
    let mut sleep = clock.sleep(10 * SECOND);
    clock.set(at(-3600));
    clock.advance(10 * SECOND);
    assert_eq!((&mut sleep).now_or_never(), None);
    clock.set(at(10));
    assert_eq!(sleep.now_or_never(), Some(()));
}

#[test]
fn every_requested_duration_is_recorded() {
    let clock = TestClock::new();
    drop(clock.sleep(2 * SECOND));
    drop(clock.sleep(Duration::ZERO));
    drop(clock.sleep(SECOND));
    assert_eq!(clock.requested_sleeps(), [2 * SECOND, Duration::ZERO, SECOND]);
}

#[test]
fn a_timeout_fires_when_the_clock_passes_it() {
    let clock = TestClock::new();
    let mut timeout = Box::pin(clock.timeout(30 * SECOND, pending::<()>()));
    assert!((&mut timeout).now_or_never().is_none());
    clock.advance(30 * SECOND);
    let err = timeout.now_or_never().unwrap().unwrap_err();
    assert!(matches!(err, StdxError::TimedOut { after } if after == 30 * SECOND), "{err:?}");
}

#[tokio::test]
async fn advance_wakes_a_task_that_waits_on_the_clock() {
    let clock = TestClock::new();
    let shared = clock.shared();
    let task = tokio::spawn(async move {
        shared.sleep(60 * SECOND).await;
        shared.now()
    });
    clock.wait_for_sleeps(1).await;
    clock.advance(60 * SECOND);
    assert_eq!(task.await.unwrap(), at(60));
}

#[tokio::test]
async fn wait_for_sleeps_returns_at_once_when_enough_sleeps_wait() {
    let clock = TestClock::new();
    let _first = clock.sleep(SECOND);
    let _second = clock.sleep(SECOND);
    clock.wait_for_sleeps(2).await;
    clock.wait_for_sleeps(0).await;
}

#[test]
fn debug_shows_the_time_and_the_waiting_sleeps() {
    let clock = TestClock::new();
    let _sleep = clock.sleep(SECOND);
    assert_eq!(format!("{clock:?}"), "TestClock { now: 2026-10-04T12:00:00Z, pending_sleeps: 1 }");
}
