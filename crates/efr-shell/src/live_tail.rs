//! When a running command's live tail is read, and how.
//!
//! The tail of an [`OutputUpdate`](crate::OutputUpdate) is the end of the output so far
//! as text. Output that the byte cleaner reads right (text, colours, carriage-return
//! progress bars) is cleaned at every change, which costs little. Output that moves the
//! cursor, such as a multi-line progress display redrawn with cursor-up, reads right
//! only on a screen, as the finished output does (`replay.rs`); the cleaner would show
//! every old frame. A screen costs a thread and a replay, so it is read at most once
//! per interval: a change within the interval is held, and the newest held change is
//! read when the interval has passed.

use std::time::Duration;

use bytes::Bytes;
use jiff::Timestamp;

use crate::replay::Scan;
use crate::run::Progress;

/// What to do with a change of the output.
#[derive(Debug)]
pub(crate) enum Step {
    /// Clean the bytes and publish the text now.
    Clean(Progress),
    /// Read these bytes on a screen and publish the text now.
    Screen(Bytes),
    /// The change is held; ask [`LiveTail::due`] when this much time has passed.
    Hold(Duration),
}

/// The live tail of one run.
#[derive(Debug)]
pub(crate) struct LiveTail {
    interval: Duration,
    /// When a tail was last read on a screen.
    last_screen: Option<Timestamp>,
    /// The newest change that needs a screen and waits for the interval.
    held: Option<Bytes>,
}

impl LiveTail {
    pub(crate) fn new(interval: Duration) -> Self {
        LiveTail { interval, last_screen: None, held: None }
    }

    /// Takes the newest state of the output at `now`.
    pub(crate) fn offer(&mut self, progress: Progress, now: Timestamp) -> Step {
        let window = progress.window();
        if !Scan::of(&window).wants_screen() {
            // A newer change replaces one that was held.
            self.held = None;
            return Step::Clean(progress);
        }
        let left = self.left(now);
        if left.is_zero() {
            self.held = None;
            self.last_screen = Some(now);
            Step::Screen(window)
        } else {
            self.held = Some(window);
            Step::Hold(left)
        }
    }

    /// The held change, to read on a screen now, when one is held.
    pub(crate) fn due(&mut self, now: Timestamp) -> Option<Bytes> {
        let held = self.held.take()?;
        self.last_screen = Some(now);
        Some(held)
    }

    /// How long until a screen may be read again.
    fn left(&self, now: Timestamp) -> Duration {
        let Some(last) = self.last_screen else {
            return Duration::ZERO;
        };
        // A clock that went back counts as no time passed.
        let since = Duration::try_from(now.duration_since(last)).unwrap_or_default();
        self.interval.saturating_sub(since)
    }
}

#[cfg(test)]
mod tests;
