//! The daemon-wide limit of turns that job notices start (efr's auto spec, section
//! 3.18, phase C): `jobs.max_wakes_per_hour_daemon` over every conversation, so the
//! jobs of many conversations cannot wake the model more often in all. Each actor keeps
//! the limit of its own conversation (`jobs.max_wakes_per_hour`) and asks this one last,
//! through `Toolbox::wake_allowed`, right before it starts such a turn: a yes counts the
//! start.
//!
//! The window is seeded at efrd's start from the store's wake turns of the last hour
//! (`efr_store::jobs::wake_starts_since`), so a restart does not reset it.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use jiff::{SignedDuration, Timestamp};

/// The time over which the limit counts.
const WINDOW: SignedDuration = SignedDuration::from_hours(1);

/// When the wake turns of every conversation started, within the last hour.
#[derive(Debug, Default)]
pub(crate) struct DaemonWakes {
    starts: Mutex<VecDeque<Timestamp>>,
}

impl DaemonWakes {
    /// A window that holds `starts`, the wake turns that the store recorded.
    pub(crate) fn seeded(starts: impl IntoIterator<Item = Timestamp>) -> Self {
        let mut starts: Vec<Timestamp> = starts.into_iter().collect();
        starts.sort_unstable();
        DaemonWakes { starts: Mutex::new(starts.into()) }
    }

    /// True when one more wake turn may start at `now` under `limit` per hour, and then
    /// counts it. A limit of 0 never lets one start.
    pub(crate) fn take(&self, now: Timestamp, limit: u32) -> bool {
        let since = now - WINDOW;
        let mut starts = self.starts.lock().unwrap_or_else(PoisonError::into_inner);
        while starts.front().is_some_and(|start| *start <= since) {
            starts.pop_front();
        }
        if u32::try_from(starts.len()).unwrap_or(u32::MAX) >= limit {
            return false;
        }
        starts.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests;
