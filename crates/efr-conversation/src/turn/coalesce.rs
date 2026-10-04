//! Coalescing: at most one update event per interval for a value that changes often.
//!
//! Every `assistant_message_updated` and `tool_call_output_updated` event carries the
//! whole value so far, so a subscriber can resume anywhere, and writing one per token
//! would make the log grow with the square of a message's length. The first change is
//! sent at once; later changes inside the interval are held, and the newest of them is
//! sent when the interval ends, measured on the injected clock.

use std::time::Duration;

use efr_stdx::time::Clock;
use jiff::Timestamp;

/// When the next update of one value may be sent.
#[derive(Debug, Clone)]
pub(crate) struct Coalescer {
    interval: Duration,
    last: Option<Timestamp>,
    held: bool,
}

impl Coalescer {
    pub(crate) fn new(interval: Duration) -> Self {
        Coalescer { interval, last: None, held: false }
    }

    /// A change happened at `now`: true when it is sent now, false when it is held.
    pub(crate) fn offer(&mut self, now: Timestamp) -> bool {
        if self.since_last(now).is_none_or(|since| since >= self.interval) {
            self.flushed(now);
            true
        } else {
            self.held = true;
            false
        }
    }

    /// How long until a held change must be sent; `None` when nothing is held.
    pub(crate) fn flush_after(&self, now: Timestamp) -> Option<Duration> {
        if !self.held {
            return None;
        }
        let since = self.since_last(now).unwrap_or(self.interval);
        Some(self.interval.saturating_sub(since))
    }

    /// The newest value was sent at `now`.
    pub(crate) fn flushed(&mut self, now: Timestamp) {
        self.last = Some(now);
        self.held = false;
    }

    fn since_last(&self, now: Timestamp) -> Option<Duration> {
        // A clock that went back counts as no time passed.
        self.last.map(|last| Duration::try_from(now.duration_since(last)).unwrap_or_default())
    }
}

/// Sleeps for `after` on `clock`, or forever when there is nothing to wait for, so a
/// `select!` branch can be switched off without a guard.
pub(crate) async fn sleep_or_pending(clock: &dyn Clock, after: Option<Duration>) {
    match after {
        Some(after) => clock.sleep(after).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
