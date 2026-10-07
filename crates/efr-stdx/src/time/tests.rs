use std::future::{pending, ready};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use pretty_assertions::assert_eq;
use tokio::sync::Notify;

use super::{Clock, Sleep, SystemClock, timestamp_from};
use crate::StdxError;

/// A clock whose timers have either all elapsed or never elapse.
#[derive(Debug)]
struct FixedClock {
    elapsed: bool,
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::UNIX_EPOCH
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        if self.elapsed { Box::pin(ready(())) } else { Box::pin(pending()) }
    }
}

/// A clock whose timers elapse when the test calls `fire`.
#[derive(Debug, Default)]
struct GatedClock {
    gate: Arc<Notify>,
}

impl GatedClock {
    fn fire(&self) {
        self.gate.notify_one();
    }
}

impl Clock for GatedClock {
    fn now(&self) -> Timestamp {
        Timestamp::UNIX_EPOCH
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        let gate = Arc::clone(&self.gate);
        Box::pin(async move { gate.notified().await })
    }
}

const AFTER: Duration = Duration::from_secs(30);

#[tokio::test]
async fn ready_future_beats_a_pending_timer() {
    let clock = FixedClock { elapsed: false };
    assert_eq!(clock.timeout(AFTER, ready(7)).await.unwrap(), 7);
}

#[tokio::test]
async fn elapsed_timer_beats_a_pending_future() {
    let clock = FixedClock { elapsed: true };
    let err = clock.timeout(AFTER, pending::<()>()).await.unwrap_err();
    assert!(matches!(err, StdxError::TimedOut { after } if after == AFTER), "{err:?}");
}

#[tokio::test]
async fn future_wins_when_both_are_ready() {
    let clock = FixedClock { elapsed: true };
    assert_eq!(clock.timeout(AFTER, ready("done")).await.unwrap(), "done");
}

#[tokio::test]
async fn timeout_works_through_an_arc_dyn_clock() {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock { elapsed: true });
    let err = clock.timeout(AFTER, pending::<()>()).await.unwrap_err();
    assert!(matches!(err, StdxError::TimedOut { .. }), "{err:?}");
}

#[tokio::test]
async fn timeout_works_through_a_dyn_reference() {
    let clock = FixedClock { elapsed: false };
    let clock: &dyn Clock = &clock;
    assert_eq!(Clock::timeout(&clock, AFTER, ready(1)).await.unwrap(), 1);
}

#[tokio::test]
async fn timeout_works_on_a_generic_clock() {
    async fn run<C: Clock>(clock: &C) -> Result<u8, StdxError> {
        clock.timeout(AFTER, ready(3)).await
    }
    assert_eq!(run(&FixedClock { elapsed: true }).await.unwrap(), 3);
}

#[test]
fn forwarding_clocks_read_the_inner_clock() {
    fn read<C: Clock>(clock: C) -> Timestamp {
        clock.now()
    }
    let inner = FixedClock { elapsed: false };
    let shared: Arc<dyn Clock> = Arc::new(FixedClock { elapsed: false });
    assert_eq!(read(&inner), Timestamp::UNIX_EPOCH);
    assert_eq!(read(shared), Timestamp::UNIX_EPOCH);
}

#[tokio::test]
async fn timer_that_fires_later_ends_the_wait() {
    let clock = GatedClock::default();
    let (_keep_sender, receiver) = tokio::sync::oneshot::channel::<u8>();
    let timeout = clock.timeout(AFTER, receiver);
    clock.fire();
    let err = timeout.await.unwrap_err();
    assert!(matches!(err, StdxError::TimedOut { .. }), "{err:?}");
}

#[tokio::test]
async fn future_that_finishes_before_the_timer_fires_wins() {
    let clock = GatedClock::default();
    let (sender, receiver) = tokio::sync::oneshot::channel::<u8>();
    let timeout = clock.timeout(AFTER, receiver);
    sender.send(9).unwrap();
    assert_eq!(timeout.await.unwrap().unwrap(), 9);
}

#[test]
fn system_clock_timeout_can_be_built_outside_a_runtime() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    drop(clock.timeout(AFTER, pending::<()>()));
    drop(SystemClock.sleep(Duration::MAX));
}

#[test]
fn timeout_of_a_send_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    assert_send(&clock.timeout(AFTER, ready(())));
}

#[tokio::test]
async fn system_clock_timeout_returns_a_ready_future_without_waiting() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    assert_eq!(clock.timeout(Duration::from_secs(3600), ready(5)).await.unwrap(), 5);
}

#[test]
fn system_clock_reads_the_wall_clock() {
    let start_of_2026 = Timestamp::from_second(1_767_225_600).unwrap();
    assert!(SystemClock.now() > start_of_2026, "the system clock is before 2026");
}

#[test]
fn system_times_in_range_convert_exactly() {
    let time = SystemTime::UNIX_EPOCH + Duration::from_millis(1_759_500_000_123);
    assert_eq!(timestamp_from(time), Timestamp::from_millisecond(1_759_500_000_123).unwrap());
}

#[test]
fn system_times_out_of_range_clamp_to_the_ends() {
    let far = Duration::from_secs(1 << 40);
    let future = SystemTime::UNIX_EPOCH.checked_add(far).unwrap();
    let past = SystemTime::UNIX_EPOCH.checked_sub(far).unwrap();
    assert_eq!(timestamp_from(future), Timestamp::MAX);
    assert_eq!(timestamp_from(past), Timestamp::MIN);
}

#[test]
fn a_stopwatch_shows_milliseconds_with_one_decimal() {
    let watch = super::Stopwatch::start();
    let shown = watch.to_string();
    let (whole, decimal) = shown.split_once('.').expect("one decimal");
    assert!(whole.chars().all(|c| c.is_ascii_digit()) && !whole.is_empty(), "{shown}");
    assert_eq!(decimal.len(), 1, "{shown}");
    assert!(watch.elapsed() < Duration::from_secs(60));
}
