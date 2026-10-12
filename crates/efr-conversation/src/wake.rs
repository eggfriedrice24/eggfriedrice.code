//! The limits of turns that job notices start (efr's auto spec, section 3.18, phase C):
//! a wake turn starts only in `auto`, at most `jobs.max_wakes_per_hour` times per hour
//! in a conversation, and runs at most [`WAKE_TURN_MAX_CALLS`] tool calls. One turn
//! takes at most [`NOTICES_PER_TURN`] notices and [`NOTICE_BYTES_PER_TURN`] bytes of
//! their text.
//!
//! Pure: every function takes the time it needs, so the tests are tables of
//! timestamps.

use std::collections::VecDeque;

use efr_protocol::Mode;
use jiff::{SignedDuration, Timestamp};

use crate::JobNoticeInput;

/// The most tool calls of one turn that a job notice started. Then the turn ends.
pub(crate) const WAKE_TURN_MAX_CALLS: usize = 20;

/// What the model reads at the end of a wake turn that reached
/// [`WAKE_TURN_MAX_CALLS`], and for each call of it that did not run.
pub(crate) const CALL_CAP: &str = "[efr: the job notice turn reached its limit of 20 tool \
                                   calls; no more calls run in it.]";

/// The most notices that one turn takes into its first message.
pub(crate) const NOTICES_PER_TURN: usize = 10;

/// The most bytes of notice text that one turn takes into its first message.
pub(crate) const NOTICE_BYTES_PER_TURN: usize = 32 * 1024;

/// The time over which the wake limits count.
const WINDOW: SignedDuration = SignedDuration::from_hours(1);

/// When the turns that notices started began, within the last hour: the limit of one
/// conversation (`jobs.max_wakes_per_hour`). The store keeps the starts, so a restart
/// seeds the window and does not reset it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WakeWindow {
    starts: VecDeque<Timestamp>,
}

impl WakeWindow {
    /// A window that holds `starts`, oldest first.
    pub(crate) fn new(starts: impl IntoIterator<Item = Timestamp>) -> Self {
        let mut starts: VecDeque<Timestamp> = starts.into_iter().collect();
        starts.make_contiguous().sort_unstable();
        WakeWindow { starts }
    }

    /// True when one more wake turn may start at `now` under `limit` per hour. A limit
    /// of 0 never lets one start.
    pub(crate) fn allows(&mut self, now: Timestamp, limit: u32) -> bool {
        let since = now - WINDOW;
        while self.starts.front().is_some_and(|start| *start <= since) {
            self.starts.pop_front();
        }
        u32::try_from(self.starts.len()).unwrap_or(u32::MAX) < limit
    }

    /// Counts a wake turn that starts at `now`.
    pub(crate) fn record(&mut self, now: Timestamp) {
        self.starts.push_back(now);
    }
}

/// The mode of a wake turn: the lowest of the mode of the turn that started the job,
/// the mode of the newest turn that a prompt started, and the config's mode now. A
/// lower mode that the user chose after the job started reaches its notices.
pub(crate) fn wake_mode(start: Mode, newest_prompt: Mode, config: Mode) -> Mode {
    start.min(newest_prompt).min(config)
}

/// The line that stands for the notices that a turn could not take.
pub(crate) fn more_notices(count: usize) -> String {
    match count {
        1 => "1 more job notice; call jobs to read it".to_owned(),
        count => format!("{count} more job notices; call jobs to read them"),
    }
}

/// Takes from `waiting` the notices that one turn takes, oldest first: at most
/// [`NOTICES_PER_TURN`] and [`NOTICE_BYTES_PER_TURN`] bytes of text. The rest stay.
pub(crate) fn take_notices(waiting: &mut Vec<JobNoticeInput>) -> Vec<JobNoticeInput> {
    let mut bytes = 0;
    let mut count = 0;
    for notice in waiting.iter() {
        let next = bytes + notice.text.len();
        // NOTE: one notice is far smaller than the limit, and a turn always takes the
        // first, so a notice can never wait for ever.
        if count == NOTICES_PER_TURN || (count > 0 && next > NOTICE_BYTES_PER_TURN) {
            break;
        }
        bytes = next;
        count += 1;
    }
    waiting.drain(..count).collect()
}

#[cfg(test)]
mod tests;
