//! The newest words of a text that grows at its end, for the detail at the end of the
//! status row: the reasoning of the model without a title (`reasoning`), and the input
//! of a tool call that the model writes ([`ToolInput`]), such as a long shell command.
//!
//! The words show only when the text has grown for [`WORDS_AFTER`]: a short text comes
//! in one piece and would show for a moment and go. They are whole words on one line,
//! in at most [`WORDS_COLUMNS`] columns, and their start moves [`WORDS_STEP`] words at a
//! time, so they stand still while they grow.
//!
//! The times come from the frames, as those of the status row do (`status`): a text
//! starts to grow at the first frame that draws it and grows until the newest one.

use std::time::Duration;

use efr_render::{WidthMethod, text_width};
use jiff::Timestamp;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::follow::since_then;
use crate::format;

/// How long a text grows before the status row shows its newest words.
pub(crate) const WORDS_AFTER: Duration = Duration::from_secs(1);

/// The most columns of the newest words in the status row, with the `…` before them.
const WORDS_COLUMNS: usize = 50;

/// The words by which the start of the newest words moves when they are full.
const WORDS_STEP: usize = 4;

/// The newest bytes of a text that its words come from.
const TAIL: usize = 2048;

/// A text that grows at its end: how far it came, its newest bytes, and the frames that
/// drew it.
#[derive(Debug, Default)]
pub(super) struct Growing {
    /// The end of the text that came, in bytes: a part that repeats bytes before it
    /// adds only the rest.
    end: u64,
    /// The newest bytes of the text, at most twice [`TAIL`].
    tail: String,
    /// The time of the first frame that drew it.
    since: Option<Timestamp>,
    /// The time of the newest frame that drew it.
    seen: Option<Timestamp>,
}

impl Growing {
    /// A text whose first part starts at byte `offset`.
    pub(super) fn from(offset: u64) -> Self {
        Growing { end: offset, ..Growing::default() }
    }

    /// Takes `delta` at byte `offset` of the whole text and keeps the part of it that
    /// is new, which it gives back. A part after a gap, which a lost draft leaves, is
    /// kept after a space, so the words on both sides stay apart.
    pub(super) fn take<'a>(&mut self, offset: u64, delta: &'a str) -> &'a str {
        let held = usize::try_from(self.end.saturating_sub(offset)).unwrap_or(usize::MAX);
        let fresh = if held == 0 { delta } else { delta.get(held..).unwrap_or_default() };
        if offset > self.end && !self.tail.is_empty() && !fresh.is_empty() {
            self.push(" ");
        }
        self.end = self.end.max(offset.saturating_add(delta.len() as u64));
        self.push(fresh);
        fresh
    }

    fn push(&mut self, text: &str) {
        self.tail.push_str(text);
        if self.tail.len() > 2 * TAIL {
            let cut = ceil(&self.tail, self.tail.len() - TAIL);
            self.tail.drain(..cut);
        }
    }

    /// The frame at `now` draws the text.
    pub(super) fn at(&mut self, now: Timestamp) {
        self.since.get_or_insert(now);
        self.seen = Some(now);
    }

    /// How long the text has grown: from the first frame that drew it to the newest;
    /// `None` before a frame drew it.
    pub(super) fn ran(&self) -> Option<Duration> {
        Some(since_then(self.since?, self.seen?))
    }

    /// The newest words of the text, made plain by `plain`, which gets the newest bytes
    /// and gives words that one space parts: `None` until the text has grown for
    /// [`WORDS_AFTER`], and for a text with no words.
    pub(super) fn words(
        &self,
        plain: impl FnOnce(&str) -> String,
        method: WidthMethod,
    ) -> Option<String> {
        if self.ran()? < WORDS_AFTER {
            return None;
        }
        let words = plain(&self.tail);
        (!words.is_empty()).then(|| newest(&words, WORDS_COLUMNS, method))
    }
}

/// The input of the tool call that the status row shows, as the model writes it.
#[derive(Debug)]
pub(super) struct ToolInput {
    /// The position of the call in the answer of the model.
    call: u32,
    input: Growing,
}

impl ToolInput {
    /// The input of call `call`, before any of it came.
    pub(super) fn new(call: u32) -> Self {
        ToolInput { call, input: Growing::default() }
    }

    /// The position of the call in the answer of the model.
    pub(super) fn call(&self) -> u32 {
        self.call
    }

    /// Takes `tail`, the newest part of the input, which ends at byte `bytes`.
    pub(super) fn take(&mut self, bytes: u64, tail: &str) {
        let offset = bytes.saturating_sub(tail.len() as u64);
        if self.input.tail.is_empty() {
            self.input.end = offset;
        }
        self.input.take(offset, tail);
    }

    /// The frame at `now` draws the input.
    pub(super) fn at(&mut self, now: Timestamp) {
        self.input.at(now);
    }

    /// The newest words of the input, for the detail of the status row: `None` until
    /// it has grown for [`WORDS_AFTER`].
    pub(super) fn words(&self, method: WidthMethod) -> Option<String> {
        self.input.words(input_words, method)
    }
}

/// The words of the input `text` on one line and safe to print. The escapes of a JSON
/// string that make space (`\n`, `\r`, `\t`) are spaces, `\"` and `\\` are their
/// characters, and each run of white space is one space. The JSON itself is not parsed:
/// the text is the raw input, and a freeform input, such as a patch, reads the same way.
fn input_words(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            plain.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'r' | 't') => plain.push(' '),
            Some(escaped @ ('"' | '\\' | '/')) => plain.push(escaped),
            Some(other) => {
                plain.push('\\');
                plain.push(other);
            }
            None => plain.push('\\'),
        }
    }
    let mut out = String::new();
    for word in plain.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format::one_line(word));
    }
    out
}

/// The newest words of `text`, whose words one space parts, that fit in `columns`,
/// after a `…` when the start is cut off. The cut is at a word, and the start moves
/// [`WORDS_STEP`] words at a time: while words come, the text grows at its end and
/// stands still, and when it is full its start jumps on. A word wider than the room is
/// cut at its start.
fn newest(text: &str, columns: usize, method: WidthMethod) -> String {
    if text_width(text, method) <= columns {
        return text.to_owned();
    }
    // The `…` and the space after it.
    let room = columns.saturating_sub(2);
    let words: Vec<&str> = text.split(' ').collect();
    // The first word from which the rest fits.
    let mut fits = words.len();
    let mut used = 0;
    for (at, word) in words.iter().enumerate().rev() {
        let width = text_width(word, method) + usize::from(at + 1 < words.len());
        if used + width > room {
            break;
        }
        used += width;
        fits = at;
    }
    let start = fits.div_ceil(WORDS_STEP) * WORDS_STEP;
    if start < words.len() {
        return format!("\u{2026} {}", words[start..].join(" "));
    }
    if fits < words.len() {
        return format!("\u{2026} {}", words[fits..].join(" "));
    }
    newest_cells(text, columns, method)
}

/// The end of `text` that fits in `columns` cells, after a `…`: for a last word that
/// is wider than the room.
fn newest_cells(text: &str, columns: usize, method: WidthMethod) -> String {
    let room = columns.saturating_sub(1);
    let pieces: Vec<(usize, &str)> = match method {
        WidthMethod::Grapheme => text.grapheme_indices(true).collect(),
        _ => text.char_indices().map(|(at, c)| (at, &text[at..at + c.len_utf8()])).collect(),
    };
    let mut start = text.len();
    let mut used = 0;
    for (at, piece) in pieces.into_iter().rev() {
        let width = text_width(piece, method);
        if used + width > room {
            break;
        }
        used += width;
        start = at;
    }
    format!("\u{2026}{}", &text[start..])
}

/// The largest index of `text` at or below `at` that starts a character.
pub(super) fn floor(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The smallest index of `text` at or above `at` that starts a character.
fn ceil(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests;
