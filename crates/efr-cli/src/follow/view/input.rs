//! The input row of a followed turn as the view keeps it: the line the user types, the
//! steers that no model call read yet, and the prompts that this view queued.
//!
//! The row is the last part of the live zone, below the status row: `› ` and the text,
//! or, while it is empty, a blank cell for the cursor and then a muted hint. A long
//! text goes on in the next row, at most [`MAX_ROWS`] rows show, and the rows around
//! the cursor are the ones that show. Above the status row, each unread steer shows as
//! `↳ steer: <first line>` and each queued prompt as `↳ queued: <first line>`, muted. A
//! steer that a model call read goes to the scrollback as the user's message, as
//! `efr history` shows a prompt.

use efr_protocol::{Seq, TurnId};
use efr_render::RenderOptions;

use crate::format::{self, Tone};
use crate::live::{Cursor, Tail};
use crate::row::RowLine;

/// The most rows of text that the input row shows.
pub(crate) const MAX_ROWS: usize = 5;

/// What starts the input row.
const MARK: &str = "\u{203a}";

/// The hint in an empty input row.
pub(crate) const HINT: &str = "enter steer \u{b7} tab queue \u{b7} esc interrupt";

/// What starts the line of an unread steer.
const STEER: &str = "\u{21b3} steer: ";

/// What starts the line of a queued prompt.
const QUEUED: &str = "\u{21b3} queued: ";

/// The note after a queued prompt that was sent as a steer too late.
const LATE: &str = " (too late to steer, so it waits in the queue)";

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
    /// what waits for the turn. Empty when nothing does.
    pub(crate) fn pending(&self, options: &RenderOptions, width: u16) -> String {
        let columns = usize::from(width);
        let method = options.width_method();
        let mut out = String::new();
        let mut line = |text: String| {
            let text = format::cut(&text, columns, method);
            out.push_str(&format::paint(&text, Tone::Dim, options));
            out.push('\n');
        };
        if let Some(prompt) = &self.prompt {
            line(format!("{QUEUED}{}", first_line(prompt)));
        }
        for steer in &self.steers {
            line(format!("{STEER}{}", first_line(&steer.text)));
        }
        for queued in &self.queued {
            let note = if queued.late { LATE } else { "" };
            line(format!("{QUEUED}{}{note}", first_line(&queued.text)));
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
        let mark = format!("{} ", format::paint(MARK, Tone::Accent, options));
        if self.line.is_empty() {
            // NOTE: the hint starts one column after the cursor. A block cursor on the
            // first letter of the hint looks like it covers that letter, and the user
            // keeps the cursor shape that they chose.
            let hint = format::cut(HINT, columns.saturating_sub(1).max(1), method);
            let text = format!("{mark} {}\n", format::paint(&hint, Tone::Dim, options));
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

/// The first line of `text` with text in it, safe to print, with ` …` when more lines
/// follow.
fn first_line(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = format::one_line(lines.next().unwrap_or(""));
    if lines.next().is_some() { format!("{first} \u{2026}") } else { first }
}

/// `text` as the user's message, as `efr history` shows a prompt: each line after
/// `> `, in bold.
pub(crate) fn user_message(text: &str, options: &RenderOptions) -> String {
    let mut out = String::new();
    for line in format::lines(text.trim_end()).lines() {
        out.push_str(&format::paint(&format!("> {line}"), Tone::Bold, options));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
