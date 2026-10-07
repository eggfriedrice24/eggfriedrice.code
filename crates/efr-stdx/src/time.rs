//! Injected time.
//!
//! Anything that schedules, retries or expires takes a [`Clock`], so a test can drive
//! a token refresh or a lease expiry with a manual clock instead of waiting.
//! [`SystemClock`] is the production clock and the only code in the workspace that
//! may call `SystemTime::now` or `tokio::time::sleep`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use tokio::time::Instant;

use crate::StdxError;

/// The future that [`Clock::sleep`] returns. It is boxed so that `Clock` works as
/// `dyn Clock`, and `'static` so that it does not borrow the clock.
pub type Sleep = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// A source of wall-clock time and timers.
///
/// Code that needs time holds an `Arc<dyn Clock>`: [`SystemClock`] in production and a
/// manual clock in tests.
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// use efr_stdx::time::{Clock, SystemClock};
///
/// async fn answer(clock: Arc<dyn Clock>) -> Result<u32, efr_stdx::StdxError> {
///     clock.timeout(Duration::from_secs(5), async { 42 }).await
/// }
/// # let _ = answer(Arc::new(SystemClock));
/// ```
pub trait Clock: Send + Sync + fmt::Debug {
    /// The current wall-clock time.
    fn now(&self) -> Timestamp;

    /// A future that completes when `duration` has passed on this clock.
    fn sleep(&self, duration: Duration) -> Sleep;

    /// Runs `future` until it completes or until `duration` passes on this clock. When
    /// both happen at the same poll, the result of `future` wins.
    ///
    /// A generic method cannot go through a vtable, so a bare `dyn Clock` cannot call
    /// this. `Arc<dyn Clock>` and `&dyn Clock` are clocks themselves (see the
    /// implementations below), so `arc_clock.timeout(..)` works, and so does
    /// `Clock::timeout(&dyn_ref, ..)`.
    fn timeout<F: Future>(&self, duration: Duration, future: F) -> Timeout<F>
    where
        Self: Sized,
    {
        Timeout::new(self.sleep(duration), duration, future)
    }
}

// NOTE: these forwarding implementations exist for `timeout`, which needs a sized
// receiver. An inherent `timeout` on `dyn Clock` is not an option: the compiler finds
// it and the trait method at the same step and reports the call as ambiguous.
impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now(&self) -> Timestamp {
        (**self).now()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        (**self).sleep(duration)
    }
}

impl<C: Clock + ?Sized> Clock for &C {
    fn now(&self) -> Timestamp {
        (**self).now()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        (**self).sleep(duration)
    }
}

/// The future that [`Clock::timeout`] returns. Its output is the output of the inner
/// future, or [`StdxError::TimedOut`] when the clock reaches the deadline first.
#[must_use = "a timeout does nothing unless it is awaited"]
pub struct Timeout<F> {
    // NOTE: boxing the inner future keeps `Timeout` `Unpin`, so `poll` needs no pin
    // projection, which safe Rust cannot express without a macro crate.
    future: Pin<Box<F>>,
    sleep: Sleep,
    after: Duration,
}

impl<F> Timeout<F> {
    fn new(sleep: Sleep, after: Duration, future: F) -> Self {
        Timeout { future: Box::pin(future), sleep, after }
    }
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, StdxError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if let Poll::Ready(output) = this.future.as_mut().poll(cx) {
            return Poll::Ready(Ok(output));
        }
        this.sleep.as_mut().poll(cx).map(|()| Err(StdxError::TimedOut { after: this.after }))
    }
}

impl<F> fmt::Debug for Timeout<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timeout").field("after", &self.after).finish_non_exhaustive()
    }
}

/// The real clock: the system wall clock and tokio timers.
///
/// A [`Sleep`] from this clock must be awaited inside a tokio runtime with the time
/// driver on; the runtimes of `efrd` and `efr` have it. It may be built anywhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[expect(
        clippy::disallowed_methods,
        reason = "SystemClock is the one place that reads the wall clock"
    )]
    fn now(&self) -> Timestamp {
        timestamp_from(SystemTime::now())
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "SystemClock is the one place that starts a real timer"
    )]
    fn sleep(&self, duration: Duration) -> Sleep {
        // A plain `tokio::time::sleep(duration)` panics when it is built outside the
        // runtime. Here the deadline is fixed at the call and the timer registers at
        // the first poll, so a `Sleep` or a `Timeout` can be built on any thread (a
        // screen thread, for example) and awaited in the runtime.
        let Some(deadline) = Instant::now().checked_add(duration) else {
            // A deadline that `Instant` cannot hold never arrives.
            return Box::pin(std::future::pending());
        };
        Box::pin(async move {
            tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
        })
    }
}

/// `Clock::now` cannot fail, so a system time outside the range of `Timestamp` (the
/// years -9999 to 9999) becomes the nearest end of that range.
fn timestamp_from(time: SystemTime) -> Timestamp {
    Timestamp::try_from(time).unwrap_or_else(|_| {
        if time < SystemTime::UNIX_EPOCH { Timestamp::MIN } else { Timestamp::MAX }
    })
}

/// How long a phase of work took, for a debug line such as
/// `phase="plan_lock" elapsed_ms=0.4`: its [`Display`](fmt::Display) is the time since
/// [`Stopwatch::start`] in milliseconds, with one decimal.
///
/// NOTE: it reads the monotonic clock and schedules nothing, so it takes no [`Clock`];
/// a test's manual clock does not move it, and no test reads it.
#[derive(Debug, Clone, Copy)]
pub struct Stopwatch {
    start: std::time::Instant,
}

impl Stopwatch {
    /// Starts measuring now.
    pub fn start() -> Stopwatch {
        Stopwatch { start: std::time::Instant::now() }
    }

    /// The time since the start.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl fmt::Display for Stopwatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1}", self.elapsed().as_secs_f64() * 1000.0)
    }
}

#[cfg(test)]
mod tests;
