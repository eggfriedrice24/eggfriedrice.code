//! The reasoning of the model on a terminal. While the model thinks, the status row
//! says `thinking`, with the bold title of the reasoning when it has one. A reasoning
//! without a title, such as the reasoning of a Claude model, puts its newest words in
//! that place ([`Reasoning::words`]), by the rules of `newest`: whole words in at most
//! 50 columns, only when the phase has run for 1 s (the summary of a short phase comes
//! in one piece just before the tool call that follows it, so its words would show for
//! a moment and go), and a start that moves a few words at a time, so the words stand
//! still while they grow. No row of the live zone holds the reasoning:
//! a row that comes and goes moves the status row. When a phase of reasoning ends
//! and it took [`ROW_AFTER`] or more, one row stays in the scrollback, at column 0:
//! `▸ <title>  thought 6s`, the mark in the `accent` role, the bold title of the
//! reasoning in the `text` role and the time muted. Without a title, the first words of
//! the reasoning stand in its place, in the `text` role and in italic, cut with `…`
//! after [`FIRST_WORDS`] columns; with no words, the row is `▸ thought 3s`
//! (TM-LongTurn 3, 4). A shorter phase leaves nothing. Bold and italic are no colour, so
//! they stay with `NO_COLOR`.
//!
//! The view only draws the reasoning: it keeps the start and the end of the phase that
//! runs, and nothing of it goes to the model or to the store from here.
//!
//! The times come from the frames, as those of the status row do (`status`): a phase
//! starts at the first frame that draws it and lasts until the newest frame before it
//! ends. The follow loop draws at least once a tick while the turn runs, so the time
//! is off by less than a tick.

use std::time::Duration;

use efr_render::{Glyphs, RenderOptions, WidthMethod, text_width};
use jiff::Timestamp;

use super::newest::{Growing, floor};
use crate::format::{self, Tone};

/// The shortest phase of reasoning that leaves a row in the scrollback.
pub(crate) const ROW_AFTER: Duration = Duration::from_secs(2);

/// The columns of the first words of a reasoning without a title, with the `…`: the
/// cut falls inside a word (TM-LongTurn 4).
const FIRST_WORDS: usize = 59;

/// The columns that the title or the first words keep at least; on a narrower screen
/// the row is only the mark and the time.
const MIN_LABEL: usize = 8;

/// The first bytes of a phase that the row looks at for its first words.
const HEAD: usize = 512;

/// The reasoning of the followed turn.
#[derive(Debug, Default)]
pub(crate) struct Reasoning {
    /// The phase that runs now. In a box, so the view stays small in the futures that
    /// hold it.
    phase: Option<Box<Phase>>,
}

/// One phase of reasoning: from its first draft to the text, the tool call or the end
/// that follows it.
#[derive(Debug, Default)]
struct Phase {
    /// Its text, from the reasoning of the turn that the drafts of the phase brought,
    /// and the frames that drew it.
    text: Growing,
    /// The first bytes of its text, at most [`HEAD`].
    head: String,
    /// The newest bold title, safe to print.
    title: Option<String>,
}

impl Reasoning {
    /// Takes the reasoning `delta` at byte `offset` of the reasoning of the turn, under
    /// `title`, the newest bold title, already safe to print. A phase starts when none
    /// runs. True when what shows changed.
    pub(crate) fn take(&mut self, offset: u64, delta: &str, title: Option<String>) -> bool {
        let started = self.phase.is_none();
        let phase = self.phase.get_or_insert_with(|| {
            Box::new(Phase { text: Growing::from(offset), ..Phase::default() })
        });
        let fresh = phase.text.take(offset, delta);
        let titled = title.is_some() && title != phase.title;
        if title.is_some() {
            phase.title = title;
        }
        if !fresh.is_empty() {
            phase.keep_head(fresh);
        }
        started || titled || !fresh.is_empty()
    }

    /// True while a phase of reasoning runs.
    pub(crate) fn running(&self) -> bool {
        self.phase.is_some()
    }

    /// The frame at `now` draws the phase that runs.
    pub(crate) fn at(&mut self, now: Timestamp) {
        if let Some(phase) = &mut self.phase {
            phase.text.at(now);
        }
    }

    /// The newest words of the phase that runs, for the status row. `None` for a phase
    /// with a title, which the status row shows in their place, before the first words,
    /// and before the phase has run for [`WORDS_AFTER`](super::newest::WORDS_AFTER).
    pub(crate) fn words(&self, more: &str, method: WidthMethod) -> Option<String> {
        let phase = self.phase.as_ref()?;
        if phase.title.is_some() {
            return None;
        }
        phase.text.words(|tail| plain(tail, true), more, method)
    }

    /// Ends the phase that runs. The row it leaves, with its newline, when it took
    /// [`ROW_AFTER`] or more.
    pub(crate) fn end(&mut self, options: &RenderOptions) -> Option<String> {
        let phase = self.phase.take()?;
        let took = phase.text.ran()?;
        (took >= ROW_AFTER).then(|| phase.row(took, options))
    }
}

impl Phase {
    /// Adds `text` to the start that the phase keeps, while it is short.
    fn keep_head(&mut self, text: &str) {
        if self.head.len() < HEAD {
            let room = floor(text, HEAD - self.head.len());
            self.head.push_str(&text[..room]);
        }
    }

    /// The row of a phase that took `took`, at the width of `options`, with its newline.
    fn row(&self, took: Duration, options: &RenderOptions) -> String {
        let method = options.width_method();
        let glyphs = options.glyphs();
        let columns = usize::from(options.width());
        let time = format!("thought {}", format::elapsed(took));
        // The mark and a space, the label, two spaces and the time.
        let mark_width = text_width(glyphs.thought, method);
        let room = columns.saturating_sub(mark_width + 1 + 2 + text_width(&time, method));
        let label = if room < MIN_LABEL {
            None
        } else if let Some(title) = &self.title {
            Some(format::paint(&cut(title, room, glyphs, method), Tone::TextBold, options))
        } else {
            let words = cut(&plain(&self.head, false), room.min(FIRST_WORDS), glyphs, method);
            (!words.is_empty()).then(|| format::paint(&words, Tone::TextItalic, options))
        };
        let mut row = format::paint(glyphs.thought, Tone::Accent, options);
        row.push(' ');
        match label {
            Some(label) => {
                row.push_str(&label);
                row.push_str("  ");
                row.push_str(&format::paint(&time, Tone::Dim, options));
            }
            None => {
                let room = columns.saturating_sub(mark_width + 1).max(1);
                row.push_str(&format::paint(&format::cut(&time, room, method), Tone::Dim, options));
            }
        }
        row.push('\n');
        row
    }
}

/// The words of reasoning `text` on one line and safe to print, without the lines of
/// its bold titles (the status row shows the newest one) and without the marks of
/// Markdown. With `live`, a title that is still open on the last line is left out
/// too, so it does not show until the status row takes it.
fn plain(text: &str, live: bool) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        let open = live && lines.peek().is_none() && line.starts_with("**");
        if line.is_empty() || is_title(line) || (open && !line[2..].contains("**")) {
            continue;
        }
        for word in unmarked(line).split_whitespace() {
            let word = word.replace("__", "").replace(['*', '`'], "");
            if word.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format::one_line(&word));
        }
    }
    out
}

/// True for a line that is all bold, such as `**Reading the logs**`: a title.
fn is_title(line: &str) -> bool {
    line.len() > 4 && line.starts_with("**") && line.ends_with("**")
}

/// `line` without the mark of a heading, a quote or a list item at its start.
fn unmarked(line: &str) -> &str {
    let Some((mark, rest)) = line.split_once(char::is_whitespace) else {
        return line;
    };
    let heading = !mark.is_empty() && mark.len() <= 6 && mark.chars().all(|c| c == '#');
    let numbered = mark
        .strip_suffix('.')
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
    if heading || numbered || matches!(mark, ">" | "-" | "+" | "*") {
        rest.trim_start()
    } else {
        line
    }
}

/// `text` cut to `columns` with the mark of more text of `glyphs` at the cut, without a
/// space before it.
fn cut(text: &str, columns: usize, glyphs: &Glyphs, method: WidthMethod) -> String {
    if text_width(text, method) <= columns {
        return text.to_owned();
    }
    let more = glyphs.more;
    let room = columns.saturating_sub(text_width(more, method));
    let mut kept = String::new();
    let mut used = 0;
    for c in text.chars() {
        let width = text_width(&c.to_string(), method);
        if used + width > room {
            break;
        }
        used += width;
        kept.push(c);
    }
    format!("{}{more}", kept.trim_end())
}

#[cfg(test)]
mod tests;
