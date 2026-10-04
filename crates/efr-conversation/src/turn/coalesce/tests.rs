use std::time::Duration;

use efr_test_support::TestClock;
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{Coalescer, sleep_or_pending};

const START: Timestamp = TestClock::START;

fn at(millis: i64) -> Timestamp {
    START.checked_add(SignedDuration::from_millis(millis)).expect("in range")
}

#[test]
fn the_first_change_is_sent_at_once() {
    let mut coalescer = Coalescer::new(Duration::from_millis(200));
    assert!(coalescer.offer(at(0)));
    assert_eq!(coalescer.flush_after(at(0)), None);
}

#[test]
fn a_change_inside_the_interval_is_held_until_it_ends() {
    let mut coalescer = Coalescer::new(Duration::from_millis(200));
    assert!(coalescer.offer(at(0)));
    assert!(!coalescer.offer(at(50)));
    assert_eq!(coalescer.flush_after(at(50)), Some(Duration::from_millis(150)));
    assert_eq!(coalescer.flush_after(at(250)), Some(Duration::ZERO));
    coalescer.flushed(at(250));
    assert_eq!(coalescer.flush_after(at(250)), None);
}

#[test]
fn a_change_after_the_interval_is_sent_at_once() {
    let mut coalescer = Coalescer::new(Duration::from_millis(200));
    assert!(coalescer.offer(at(0)));
    assert!(coalescer.offer(at(200)));
    assert!(!coalescer.offer(at(300)));
}

#[test]
fn a_zero_interval_sends_every_change() {
    let mut coalescer = Coalescer::new(Duration::ZERO);
    assert!(coalescer.offer(at(0)));
    assert!(coalescer.offer(at(0)));
}

#[test]
fn a_clock_that_went_back_counts_as_no_time_passed() {
    let mut coalescer = Coalescer::new(Duration::from_millis(200));
    assert!(coalescer.offer(at(1000)));
    assert!(!coalescer.offer(at(0)));
    assert_eq!(coalescer.flush_after(at(0)), Some(Duration::from_millis(200)));
}

#[tokio::test]
async fn sleep_or_pending_sleeps_on_the_clock() {
    let clock = TestClock::new();
    let sleeper = clock.clone();
    let task = tokio::spawn(async move {
        sleep_or_pending(&sleeper, Some(Duration::from_secs(1))).await;
    });
    clock.wait_for_sleeps(1).await;
    clock.advance(Duration::from_secs(1));
    task.await.expect("the sleep ends");
}

#[tokio::test]
async fn sleep_or_pending_without_a_duration_never_ends() {
    let clock = TestClock::new();
    let mut never = Box::pin(sleep_or_pending(&clock, None));
    assert!(futures::poll!(never.as_mut()).is_pending());
    assert_eq!(clock.pending_sleeps(), 0, "nothing was scheduled");
}
