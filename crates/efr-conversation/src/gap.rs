//! The gap before each model call of one conversation, for the log.
//!
//! A provider's prompt cache keeps an entry for a time to live that counts from the
//! start of the request that wrote or last read it. So the time from the start of the
//! conversation's previous model call to the start of this one says whether the
//! entries of the previous call can still be there. The turn's calls, its compaction's
//! summary call and a manual compaction's summary call all count, because each one
//! reads and writes the same cache. The gap goes into the `gap_ms` field of the
//! call's `provider_request` span, so every line of the call carries it, also the
//! provider's own line about its cache markers (`efr-provider-anthropic`). This is the
//! measurement of decision 6 of the Claude provider plan: which time to live fits the
//! pauses of a shell agent. Nothing else reads it.
//!
//! The first call after a start of efrd has no gap: the time of the call before is not
//! stored.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use jiff::Timestamp;

/// When the conversation's newest model call started.
#[derive(Debug, Default)]
pub(crate) struct CallGap {
    started: Mutex<Option<Timestamp>>,
}

impl CallGap {
    /// Notes a model call that starts at `now` and returns the time since the start of
    /// the call before it; `None` for the first call since efrd started. A clock that
    /// went back gives a gap of zero.
    pub(crate) fn start(&self, now: Timestamp) -> Option<Duration> {
        let mut started = self.started.lock().unwrap_or_else(PoisonError::into_inner);
        let before = started.replace(now)?;
        Some(Duration::try_from(now.duration_since(before)).unwrap_or(Duration::ZERO))
    }
}

/// `gap` in whole milliseconds, the value of the `gap_ms` span field.
pub(crate) fn millis(gap: Duration) -> u64 {
    u64::try_from(gap.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
