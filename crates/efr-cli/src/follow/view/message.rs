//! The assistant message that streams now: the text that arrived, and the part of it
//! that went into the renderer.
//!
//! Text arrives as drafts about every 16 ms, or as a persisted update every 200 ms from
//! a daemon without drafts or after a draft was dropped. All text that arrived goes into
//! the renderer at the next frame, so it shows within one frame of its arrival. What
//! streams is exactly what `render` makes of the whole text.

use efr_render::{RenderOptions, Renderer};

use crate::live::Measured;

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
            Some(delta) => self.received.push_str(delta),
            None if self.received.starts_with(text) => {}
            None => tracing::debug!(index = self.index, "an assistant message rewrote sent text"),
        }
    }

    /// Pushes all of the waiting text into the renderer and returns what it committed.
    pub(crate) fn push_all(&mut self) -> String {
        let Some(delta) = self.received.get(self.pushed..).filter(|delta| !delta.is_empty()) else {
            return String::new();
        };
        let update = self.renderer.push(delta);
        let committed = update.committed().to_owned();
        update.live().clone_into(&mut self.live);
        let width = self.renderer.options().width();
        self.measured = Some(Measured { rows: update.live_rows(), width });
        self.pushed = self.received.len();
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

#[cfg(test)]
mod tests;
