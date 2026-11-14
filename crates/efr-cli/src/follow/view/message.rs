//! The assistant message that streams now: the text that arrived, and the part of it
//! that went into the renderer.
//!
//! Text arrives as drafts about every 16 ms, or as a persisted update every 200 ms from
//! a daemon without drafts or after a draft was dropped. All text that arrived goes into
//! the renderer at the next frame, so it shows within one frame of its arrival. What
//! streams is exactly what `render` makes of the whole text.
//!
//! Off a terminal the renderer passes the markdown through, except a citation mark
//! (` [[n]](<url>)`, which `efr_conversation` puts after the words that a web source
//! backs): it becomes ` [n]`, as a terminal shows it, because the list of sources after
//! the answer names each URL (decision 3 of the 0.1.0 plan, TM-Web). A mark that may
//! still be open at the end of the text that arrived waits for the next text.

use efr_render::{RenderOptions, Renderer};

/// One assistant message while it streams.
#[derive(Debug)]
pub(crate) struct Message {
    pub(crate) index: u32,
    /// Boxed: the view lives in the futures of the follow, and a renderer with the state
    /// of an open fence would push them past clippy's `large_futures` limit.
    renderer: Box<Renderer>,
    /// The text of the message as far as it arrived.
    received: String,
    /// How many bytes of `received` went into the renderer.
    pushed: usize,
    /// The renderer's live zone after the last push.
    live: String,
}

impl Message {
    pub(crate) fn new(index: u32, options: RenderOptions) -> Message {
        Message {
            index,
            renderer: Box::new(Renderer::new(options)),
            received: String::new(),
            pushed: 0,
            live: String::new(),
        }
    }

    /// The text that arrived so far.
    pub(crate) fn received(&self) -> &str {
        &self.received
    }

    /// The renderer's live zone after the last push.
    pub(crate) fn live(&self) -> &str {
        &self.live
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

    /// The message's text is the text that arrived up to byte `offset`, then `delta`:
    /// the same as [`receive`](Message::receive) of that text, at the cost of the
    /// delta only. An `offset` past the text, or inside a character, changes nothing.
    pub(crate) fn receive_at(&mut self, offset: usize, delta: &str) {
        let Some(after) = self.received.get(offset..) else { return };
        match delta.strip_prefix(after) {
            Some(new) => self.received.push_str(new),
            None if after.starts_with(delta) => {}
            None => tracing::debug!(index = self.index, "an assistant message rewrote sent text"),
        }
    }

    /// Pushes all of the waiting text into the renderer and returns what it committed.
    /// Off a terminal, a citation mark that may still be open at the end waits.
    pub(crate) fn push_all(&mut self) -> String {
        self.push(false)
    }

    fn push(&mut self, last: bool) -> String {
        let Some(delta) = self.received.get(self.pushed..).filter(|delta| !delta.is_empty()) else {
            return String::new();
        };
        if !self.renderer.options().is_terminal() {
            let (plain, taken) = plain_marks(delta, last);
            self.pushed += taken;
            return self.renderer.push(&plain).committed().to_owned();
        }
        let update = self.renderer.push(delta);
        let committed = update.committed().to_owned();
        update.live().clone_into(&mut self.live);
        self.pushed = self.received.len();
        committed
    }

    /// Ends the message: everything that arrived goes into the renderer, and the rest
    /// of its output comes back.
    pub(crate) fn finish(mut self) -> String {
        let mut rest = self.push(true);
        rest.push_str(&self.renderer.finish());
        rest
    }
}

/// The start of a citation mark, up to its number.
const MARK_OPEN: &str = " [[";
/// What comes between the number of a citation mark and its URL.
const MARK_MIDDLE: &str = "]](<";

/// `text` with each citation mark ` [[n]](<url>)` written as ` [n]`, and how many bytes
/// of `text` that took. Unless `last`, a mark that the end of `text` may still complete
/// stays out: its bytes are not taken.
pub(crate) fn plain_marks(text: &str, last: bool) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(at) = text[from..].find(' ').map(|at| from + at) {
        match mark_at(&text[at..]) {
            Mark::Complete { number, len } => {
                out.push_str(&text[copied..at]);
                out.push_str(" [");
                out.push_str(number);
                out.push(']');
                copied = at + len;
                from = copied;
            }
            Mark::Open if !last => {
                out.push_str(&text[copied..at]);
                return (out, at);
            }
            Mark::Open | Mark::No => from = at + 1,
        }
    }
    out.push_str(&text[copied..]);
    (out, text.len())
}

/// What a text that starts with a blank holds at its start.
#[derive(Debug, PartialEq, Eq)]
enum Mark<'a> {
    /// A whole citation mark of `len` bytes.
    Complete { number: &'a str, len: usize },
    /// The start of a mark that the end of the text cut off.
    Open,
    /// No mark.
    No,
}

fn mark_at(text: &str) -> Mark<'_> {
    let Some(rest) = text.strip_prefix(MARK_OPEN) else {
        return cut_or_no(text, MARK_OPEN);
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (number, rest) = rest.split_at(digits);
    if rest.is_empty() {
        return Mark::Open;
    }
    if number.is_empty() {
        return Mark::No;
    }
    let Some(url) = rest.strip_prefix(MARK_MIDDLE) else {
        return cut_or_no(rest, MARK_MIDDLE);
    };
    // NOTE: the mark encodes `<`, `>`, `\`, blanks and control characters of its URL,
    // so its first `>` ends it.
    let Some(end) = url.find(|c: char| c == '>' || c.is_whitespace() || c.is_control()) else {
        return Mark::Open;
    };
    match url[end..].as_bytes() {
        [b'>'] => Mark::Open,
        [b'>', b')', ..] => Mark::Complete { number, len: text.len() - url.len() + end + 2 },
        _ => Mark::No,
    }
}

/// [`Mark::Open`] when `text` is a start of `expected` that the end cut off, else
/// [`Mark::No`].
fn cut_or_no(text: &str, expected: &str) -> Mark<'static> {
    if expected.starts_with(text) { Mark::Open } else { Mark::No }
}

#[cfg(test)]
mod tests;
