//! The assistant message that streams now: the text that arrived, the part of it that
//! went into the renderer, and how fast the rest follows.
//!
//! Text arrives in bursts: a draft about every 16 ms, or a persisted update every
//! 200 ms from a daemon without drafts, or after a draft was dropped. A burst is shown
//! over the time until the next one is due, at most [`WINDOW`], so it reads as a
//! stream instead of a jump. Only how much of the text goes to `Renderer::push` changes,
//! so what streams is still exactly what `render` makes of the whole text. All of it
//! shows at once when [`CATCH_UP_LINES`] lines or more wait, when the oldest text waited
//! [`WINDOW`], and when the message ends.

use std::time::Duration;

use efr_render::{RenderOptions, Renderer};
use jiff::Timestamp;

use crate::live::Measured;

/// The longest time over which a burst is shown, and the longest time text waits.
pub(crate) const WINDOW: Duration = Duration::from_millis(120);

/// The waiting lines that make all of the text show at once.
const CATCH_UP_LINES: usize = 8;

/// The time between two frames, the most that one frame moves the text on.
const FRAME: Duration = Duration::from_millis(16);

/// One assistant message while it streams.
#[derive(Debug)]
pub(crate) struct Message {
    pub(crate) index: u32,
    renderer: Renderer,
    /// The text of the message as far as it arrived.
    received: String,
    /// How many bytes of `received` went into the renderer.
    pushed: usize,
    /// The renderer's live zone after the last push, and its height.
    live: String,
    measured: Option<Measured>,
    pace: Pace,
}

/// How fast the waiting text goes into the renderer.
#[derive(Debug, Default)]
struct Pace {
    /// Text arrived since the last frame.
    fresh: bool,
    /// The frame that saw text arrive last time.
    arrived: Option<Timestamp>,
    /// The frame that saw the oldest waiting text arrive.
    since: Option<Timestamp>,
    /// The waiting bytes when the last burst arrived, and the time to show them over.
    burst: usize,
    window: Duration,
}

impl Message {
    pub(crate) fn new(index: u32, options: RenderOptions) -> Message {
        Message {
            index,
            renderer: Renderer::new(options),
            received: String::new(),
            pushed: 0,
            live: String::new(),
            measured: None,
            pace: Pace::default(),
        }
    }

    /// The text that arrived so far.
    pub(crate) fn received(&self) -> &str {
        &self.received
    }

    /// The renderer's live zone after the last push, and its height when the renderer
    /// counted it.
    pub(crate) fn live(&self) -> (&str, Option<Measured>) {
        (&self.live, self.measured)
    }

    /// True while text that arrived waits to go into the renderer.
    pub(crate) fn waiting(&self) -> bool {
        self.pushed < self.received.len()
    }

    /// The message's text is `text` now. Text that only repeats the start of what
    /// arrived, such as a persisted update behind the drafts already shown, changes
    /// nothing. Text that rewrites what arrived is dropped: committed output cannot be
    /// taken back.
    pub(crate) fn receive(&mut self, text: &str) {
        match text.strip_prefix(self.received.as_str()) {
            Some("") => {}
            Some(delta) => {
                self.received.push_str(delta);
                self.pace.fresh = true;
            }
            None if self.received.starts_with(text) => {}
            None => tracing::debug!(index = self.index, "an assistant message rewrote sent text"),
        }
    }

    /// Pushes all of the waiting text into the renderer and returns what it committed.
    pub(crate) fn push_all(&mut self) -> String {
        let end = self.received.len();
        self.pace = Pace { arrived: self.pace.arrived, ..Pace::default() };
        self.push_to(end)
    }

    /// Pushes the part of the waiting text that is due at `now`, the time of a frame,
    /// `since_frame` after the frame before it, and returns what the renderer committed.
    pub(crate) fn reveal(&mut self, now: Timestamp, since_frame: Option<Duration>) -> String {
        if std::mem::take(&mut self.pace.fresh) {
            // The time to show this burst over is the time since the last one, as the
            // next is due as long after it.
            let gap = self.pace.arrived.map(|arrived| since_then(arrived, now));
            self.pace.arrived = Some(now);
            self.pace.since.get_or_insert(now);
            self.pace.window = gap.map_or(Duration::ZERO, |gap| gap.min(WINDOW));
            self.pace.burst = self.received.len() - self.pushed;
        }
        if !self.waiting() {
            return String::new();
        }
        let waiting = &self.received[self.pushed..];
        let waited = self.pace.since.map_or(Duration::ZERO, |since| since_then(since, now));
        let catch_up = self.pace.window.is_zero()
            || waited >= WINDOW
            || waiting.matches('\n').count() >= CATCH_UP_LINES;
        if catch_up {
            return self.push_all();
        }
        let step = since_frame.map_or(FRAME, |step| step.min(FRAME));
        let share = u128::try_from(self.pace.burst).unwrap_or(u128::MAX) * step.as_millis();
        let due = usize::try_from(share.div_ceil(self.pace.window.as_millis().max(1)))
            .unwrap_or(usize::MAX)
            .max(1);
        let mut end = self.pushed.saturating_add(due).min(self.received.len());
        while !self.received.is_char_boundary(end) {
            end += 1;
        }
        if end == self.received.len() {
            return self.push_all();
        }
        self.push_to(end)
    }

    /// Pushes `received` up to byte `end` into the renderer.
    fn push_to(&mut self, end: usize) -> String {
        let Some(delta) = self.received.get(self.pushed..end).filter(|delta| !delta.is_empty())
        else {
            return String::new();
        };
        let update = self.renderer.push(delta);
        let committed = update.committed().to_owned();
        update.live().clone_into(&mut self.live);
        let width = self.renderer.options().width();
        self.measured = Some(Measured { rows: update.live_rows(), width });
        self.pushed = end;
        committed
    }

    /// Ends the message: everything that arrived goes into the renderer, and the rest
    /// of its output comes back.
    pub(crate) fn finish(mut self) -> String {
        let mut rest = self.push_all();
        rest.push_str(&self.renderer.finish());
        rest
    }
}

/// The time from `then` to `now`; zero when `now` is earlier.
fn since_then(then: Timestamp, now: Timestamp) -> Duration {
    Duration::try_from(now.duration_since(then)).unwrap_or_default()
}

#[cfg(test)]
mod tests;
