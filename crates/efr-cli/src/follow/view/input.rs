//! The input row of a followed turn as the view keeps it: the line the user types, the
//! steers that no model call read yet, and the prompts that this view queued.
//!
//! The row is the last part of the live zone, below the status row: `› ` and the text,
//! or, while it is empty, a blank cell for the cursor and then a muted hint. The mark is
//! in the `accent` role. While a shell call of the turn has run for a moment, the hint
//! also names Ctrl+B ([`BACKGROUND_HINT`]). A long text goes on in the next row, at most
//! [`MAX_ROWS`] rows show, and the rows around the cursor are the ones that show
//! (TM-Live 15-18, 29).
//!
//! Above the status row, each unread steer shows as `↳ steer  <first line> · not read
//! yet` and each queued prompt as `↳ queued  <first line>`, muted, at most
//! [`MAX_WAITING`] rows, then `↳ 2 more queued` (TM-Live 19). A steer that a model call
//! read goes to the scrollback as `› <text>`, the mark in the `accent` role and the text
//! in bold, with one blank line above it and one below (TM-Legend 27, TM-Sequence 12).

use efr_protocol::{Seq, TurnId};
use efr_render::{Glyphs, RenderOptions};

use crate::format::{self, Tone};
use crate::live::{Cursor, Tail};
use crate::row::RowLine;

/// The most rows of text that the input row shows.
pub(crate) const MAX_ROWS: usize = 5;

/// The keys of the hint in an empty input row, with what they do.
pub(crate) const HINT: &[&str] = &["enter steer", "tab queue", "esc interrupt"];

/// What the hint starts with while a shell call of the turn runs that Ctrl+B can move:
/// first, so a narrow screen still shows it.
pub(crate) const BACKGROUND_HINT: &str = "ctrl+b background";

/// The most rows of unread steers and queued prompts; more make one row that counts
/// them.
pub(crate) const MAX_WAITING: usize = 3;

/// The word of the row of an unread steer.
const STEER: &str = "steer";

/// The word of the row of a queued prompt.
const QUEUED: &str = "queued";

/// The note after an unread steer.
const UNREAD: &str = "not read yet";

/// The note after a queued prompt that was sent as a steer too late.
const LATE: &str = "too late to steer, waits in the queue";

/// A steer of this view that no model call read yet.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Steer {
    seq: Seq,
    text: String,
}

/// A prompt that this view queued and that did not start yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Queued {
    pub(crate) turn: TurnId,
    pub(crate) text: String,
    /// It was a steer that came too late for its turn.
    late: bool,
}

/// The input row's state.
#[derive(Debug, Default)]
pub(crate) struct Input {
    pub(crate) line: RowLine,
    steers: Vec<Steer>,
    /// In queue order.
    queued: Vec<Queued>,
    /// The prompt of the followed turn, which this view queued: it shows as queued
    /// until the turn starts, and then goes to the scrollback as the user's message.
    pub(crate) prompt: Option<String>,
    /// Keys come to the row, so it shows.
    pub(crate) shown: bool,
    /// Bracketed paste is on in the terminal.
    pub(crate) paste: bool,
    /// A shell call of the turn runs that Ctrl+B can move: the hint says so.
    pub(crate) background: bool,
}

impl Input {
    /// A steer of this view, recorded as `seq`.
    pub(crate) fn steered(&mut self, seq: Seq, text: String) {
        self.steers.push(Steer { seq, text });
    }

    /// A prompt that this view queued as `turn`; `late` for a steer that came too late.
    pub(crate) fn queued(&mut self, turn: TurnId, text: String, late: bool) {
        if !self.queued.iter().any(|queued| queued.turn == turn) {
            self.queued.push(Queued { turn, text, late });
        }
    }

    /// True when `seq` is a steer of this view that no model call read yet.
    pub(crate) fn is_steer(&self, seq: Seq) -> bool {
        self.steers.iter().any(|steer| steer.seq == seq)
    }

    /// The steers of this view among `seqs`, which a model call read: they leave the
    /// list, and their texts come back in the order of `seqs`.
    pub(crate) fn delivered(&mut self, seqs: &[Seq]) -> Vec<String> {
        let mut texts = Vec::new();
        for seq in seqs {
            if let Some(at) = self.steers.iter().position(|steer| steer.seq == *seq) {
                texts.push(self.steers.remove(at).text);
            }
        }
        texts
    }

    /// The steers among `seqs` are now the prompt of `turn`, which an interrupt sent
    /// again and which runs next. Nothing when none of them is a steer of this view.
    pub(crate) fn resent(&mut self, turn: TurnId, seqs: &[Seq]) -> bool {
        let texts = self.delivered(seqs);
        if texts.is_empty() || self.queued.iter().any(|queued| queued.turn == turn) {
            return false;
        }
        self.queued.insert(0, Queued { turn, text: texts.join("\n"), late: false });
        true
    }

    /// The unread steers of this view, by sequence number.
    pub(crate) fn unread(&self) -> Vec<Seq> {
        self.steers.iter().map(|steer| steer.seq).collect()
    }

    /// Takes the texts of the unread steers, oldest first.
    pub(crate) fn take_unread(&mut self) -> Vec<String> {
        self.steers.drain(..).map(|steer| steer.text).collect()
    }

    /// The prompts that this view queued, in queue order.
    pub(crate) fn queued_turns(&self) -> Vec<TurnId> {
        self.queued.iter().map(|queued| queued.turn).collect()
    }

    /// The newest prompt that this view queued.
    pub(crate) fn newest(&self) -> Option<TurnId> {
        self.queued.last().map(|queued| queued.turn)
    }

    /// Takes the prompt of `turn` out of the list.
    pub(crate) fn remove(&mut self, turn: TurnId) -> Option<Queued> {
        let at = self.queued.iter().position(|queued| queued.turn == turn)?;
        Some(self.queued.remove(at))
    }

    /// The prompt that runs next of those this view queued, out of the list.
    pub(crate) fn next(&mut self) -> Option<Queued> {
        (!self.queued.is_empty()).then(|| self.queued.remove(0))
    }

    /// The unread steers and the queued prompts, one muted line each, cut to `width`:
    /// what waits for the turn, at most [`MAX_WAITING`] rows and then one that counts the
    /// rest. Empty when nothing waits.
    pub(crate) fn pending(&self, options: &RenderOptions, width: u16) -> String {
        let columns = usize::from(width);
        let method = options.width_method();
        let glyphs = options.glyphs();
        let mut rows: Vec<String> = Vec::new();
        let row = |word: &str, text: &str, note: Option<&str>| {
            let mut row = format!("{} {word}  {}", glyphs.waiting, first_line(text, glyphs));
            if let Some(note) = note {
                row.push_str(&format!(" {} {note}", glyphs.separator));
            }
            row
        };
        if let Some(prompt) = &self.prompt {
            rows.push(row(QUEUED, prompt, None));
        }
        for steer in &self.steers {
            rows.push(row(STEER, &steer.text, Some(UNREAD)));
        }
        for queued in &self.queued {
            rows.push(row(QUEUED, &queued.text, queued.late.then_some(LATE)));
        }
        if rows.len() > MAX_WAITING {
            let more = rows.len() - MAX_WAITING;
            rows.truncate(MAX_WAITING);
            rows.push(format!("{} {more} more {QUEUED}", glyphs.waiting));
        }
        let mut out = String::new();
        for row in rows {
            let row = format::cut(&row, columns, method);
            out.push_str(&format::paint(&row, Tone::Dim, options));
            out.push('\n');
        }
        out
    }

    /// The input row at `width`: `› ` and the text, or the hint one column after the
    /// cursor while it is empty, and the cursor in it.
    pub(crate) fn row(&self, options: &RenderOptions, width: u16) -> Tail {
        // The mark and its blank, and one column for the cursor after the last
        // character of a full row.
        let columns = usize::from(width).saturating_sub(3).max(1);
        let method = options.width_method();
        let glyphs = options.glyphs();
        let mark = format!("{} ", format::paint(glyphs.prompt, Tone::Accent, options));
        if self.line.is_empty() {
            // NOTE: one blank column stands between the cursor and the hint (TM-Live
            // 15). A block cursor next to the first letter of the hint looks like it
            // covers that letter, and the user keeps the cursor shape that they chose.
            let hint = hint(self.background, glyphs);
            let hint = format::cut(&hint, columns.saturating_sub(2).max(1), method);
            let text = format!("{mark}  {}\n", format::paint(&hint, Tone::Dim, options));
            return Tail { text, cursor: Some(Cursor { line: 0, column: 2 }) };
        }
        let layout = self.line.layout(columns, method);
        let start = layout.cursor_row.saturating_sub(MAX_ROWS - 1);
        let mut text = String::new();
        for (at, row) in layout.rows.iter().enumerate().skip(start).take(MAX_ROWS) {
            text.push_str(if at == 0 { &mark } else { "  " });
            text.push_str(row);
            text.push('\n');
        }
        let cursor = Cursor { line: layout.cursor_row - start, column: 2 + layout.cursor_col };
        Tail { text, cursor: Some(cursor) }
    }
}

/// The hint of an empty input row, with Ctrl+B first when `background`: the keys apart
/// by the separator of `glyphs`.
pub(crate) fn hint(background: bool, glyphs: &Glyphs) -> String {
    let separator = format!(" {} ", glyphs.separator);
    let keys = background.then_some(BACKGROUND_HINT).into_iter().chain(HINT.iter().copied());
    keys.collect::<Vec<_>>().join(&separator)
}

/// The first line of `text` with text in it, safe to print, with ` …` when more lines
/// follow.
fn first_line(text: &str, glyphs: &Glyphs) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = format::one_line(lines.next().unwrap_or(""));
    if lines.next().is_some() { format!("{first} {}", glyphs.more) } else { first }
}

/// `text` as the user's steer in the scrollback: `› ` and the first line, the mark in
/// the `accent` role and the text in bold, and each later line two columns in, as the
/// input row shows them.
pub(crate) fn user_message(text: &str, options: &RenderOptions) -> String {
    let mark = format::paint(options.glyphs().prompt, Tone::Accent, options);
    let mut out = String::new();
    for (at, line) in format::lines(text.trim_end()).lines().enumerate() {
        if at == 0 {
            out.push_str(&mark);
            out.push(' ');
        } else {
            out.push_str("  ");
        }
        out.push_str(&format::paint(line, Tone::TextBold, options));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
