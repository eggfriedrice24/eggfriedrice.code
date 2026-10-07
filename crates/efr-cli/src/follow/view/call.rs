//! A tool call of the followed turn: a block in the live zone while it runs, and the
//! same block written once when it ends.
//!
//! ```text
//! · $ cargo metadata --no-deps | jq -r '...'
//!   ✓ 1.4s
//!
//! · $ make
//!   │ cc -c a.c
//!   │ error: no rule
//!   ✗ exit 2 · 1.2s
//! ```
//!
//! While the call runs, its first row carries the spinner (accent), what the call does
//! (code) cut to the width with `…` at the cut and, from 1 s on, how long it has run;
//! a few more lines of a command of several follow, then the last [`TAIL_LINES`] lines
//! of its output with text in them, each after `  │ `. The status row hides meanwhile,
//! so the screen shows one sign of work. A call whose approval waits shows nothing yet:
//! the question is on the screen.
//!
//! When the call ends, its block is written once in place of the live rows: `·` in the
//! `accent` role and what the call does in the `code` role, every line of a command on
//! its own, a long line going on in the next row after a `\`, indented; then for a
//! failed call the last lines of its output; then the result on its own row: `✓` in
//! the `success` role, or `✗ exit 101`, `✗ failed` or `✗ refused: <why>` in the
//! `error` role. The time follows for a call that ran 1 s or more.

use std::time::Duration;

use efr_protocol::CallId;
use efr_render::{RenderOptions, text_width};
use jiff::Timestamp;

use crate::follow::since_then;
use crate::format::{self, CallText, Tone, WRAP_MARK};

/// The lines of output that a running call shows and a failed one keeps.
pub(crate) const TAIL_LINES: usize = 3;

/// The time from which a call says how long it ran.
const SHOW_TIME: Duration = Duration::from_secs(1);

/// What starts a line of a call's output.
pub(crate) const BAR: &str = "  \u{2502} ";

/// What starts a call's block.
const MARK: &str = "\u{b7}";

/// What starts the result of a call and its notes, under its mark.
const INDENT: &str = "  ";

/// How much further a row that goes on is indented than the line it goes on from.
const GOES_ON: usize = 4;

/// The lines after the first that a running call shows. With more, the last of these
/// rows says how many lines follow.
const LIVE_LINES: usize = 3;

/// What joins the parts of a result, such as `✗ exit 2 · 1.2s`.
const JOIN: &str = " \u{b7} ";

/// How a call ended, for its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome<'a> {
    /// It ran and went well.
    Ran,
    /// Its command exited with this code.
    Exited { code: i32, contained: bool },
    /// The tool failed without an exit code.
    Failed { contained: bool },
    /// efr refused it before it ran, for this reason.
    Refused(&'a str),
    /// Its sandbox could not start, for this reason.
    NotStarted(&'a str),
}

impl Outcome<'_> {
    /// True when the call failed, so its last output lines stay on the screen.
    pub(crate) fn failed(self) -> bool {
        matches!(self, Outcome::Exited { .. } | Outcome::Failed { .. })
    }

    /// The result's text without the time, and its tone.
    fn result(self) -> (String, Tone) {
        let sandbox = |contained: bool| if contained { " (sandbox)" } else { "" };
        match self {
            Outcome::Ran => ("\u{2713}".to_owned(), Tone::Success),
            Outcome::Exited { code, contained } => {
                (format!("\u{2717} exit {code}{}", sandbox(contained)), Tone::Failure)
            }
            Outcome::Failed { contained } => {
                (format!("\u{2717} failed{}", sandbox(contained)), Tone::Failure)
            }
            Outcome::Refused(reason) => {
                (format!("\u{2717} refused: {}", format::one_line(reason)), Tone::Failure)
            }
            Outcome::NotStarted(reason) => (
                format!(
                    "\u{2717} the sandbox could not start: {}; efr checks it again",
                    format::one_line(reason)
                ),
                Tone::Failure,
            ),
        }
    }

    /// True when the call ran, so its time says something.
    fn ran(self) -> bool {
        !matches!(self, Outcome::Refused(_) | Outcome::NotStarted(_))
    }
}

/// A tool call of the followed turn, from its start to its end.
#[derive(Debug)]
pub(crate) struct Call {
    pub(crate) call_id: CallId,
    text: CallText,
    /// The daemon's time when it began to run: its start, or the answer to its
    /// approval.
    since: Option<Timestamp>,
    /// The time of the first frame that showed it running.
    shown: Option<Timestamp>,
    /// Its approval waits for an answer, so it does not run yet.
    pub(crate) awaiting: bool,
}

impl Call {
    /// Call `call_id`, which does `text`, started at `since` on the daemon's clock.
    pub(crate) fn new(call_id: CallId, text: CallText, since: Option<Timestamp>) -> Call {
        Call { call_id, text, since, shown: None, awaiting: false }
    }

    /// Its approval was answered at `at`: the call runs from then.
    pub(crate) fn approved(&mut self, at: Option<Timestamp>) {
        self.awaiting = false;
        if at.is_some() {
            self.since = at;
        }
        self.shown = None;
    }

    /// The frame at `now` shows the call: its time counts from the first such frame.
    pub(crate) fn show(&mut self, now: Timestamp) {
        self.shown.get_or_insert(now);
    }

    /// The name and the first line, such as `$ cargo test`.
    fn head(&self) -> String {
        match self.text.lines.first() {
            Some(first) => format!("{} {first}", self.text.name),
            None => self.text.name.clone(),
        }
    }

    /// The column where the text of each line starts: after the mark and the name.
    fn text_column(&self) -> usize {
        MARK.chars().count() + 1 + self.text.name.chars().count() + 1
    }

    /// The rows of the running call at `now`, with `spinner` and their newlines: each
    /// cut to the width with `…` at the cut.
    pub(crate) fn running(&self, spinner: char, now: Timestamp, options: &RenderOptions) -> String {
        let elapsed = self.shown.map_or(Duration::ZERO, |shown| since_then(shown, now));
        let time = (elapsed >= SHOW_TIME).then(|| format::elapsed(elapsed));
        let method = options.width_method();
        let width = usize::from(options.width());
        let mut reserve = 2;
        if let Some(time) = &time {
            reserve += 2 + text_width(time, method);
        }
        let mut out = format::paint(&spinner.to_string(), Tone::Accent, options);
        out.push(' ');
        let head = format::cut(&self.head(), width.saturating_sub(reserve), method);
        out.push_str(&format::paint(&head, Tone::Code, options));
        if let Some(time) = time {
            out.push_str("  ");
            out.push_str(&format::paint(&time, Tone::Dim, options));
        }
        out.push('\n');
        let indent = " ".repeat(self.text_column());
        let room = width.saturating_sub(indent.len());
        let more = &self.text.lines[1.min(self.text.lines.len())..];
        // A note in place of one line would take its row and show less.
        let shown = if more.len() <= LIVE_LINES { more.len() } else { LIVE_LINES - 1 };
        for line in &more[..shown] {
            out.push_str(&indent);
            out.push_str(&format::paint(&format::cut(line, room, method), Tone::Code, options));
            out.push('\n');
        }
        let hidden = more.len() - shown;
        if hidden > 0 {
            let unit = if hidden == 1 { "line" } else { "lines" };
            out.push_str(&indent);
            out.push_str(&format::paint(&format!("({hidden} more {unit})"), Tone::Dim, options));
            out.push('\n');
        }
        out
    }

    /// The rows that start the call's block: the mark and every line of what it does,
    /// each line cut into rows of the width on a terminal, with their newlines.
    pub(crate) fn header(&self, options: &RenderOptions) -> String {
        let columns = format::columns(options);
        let column = self.text_column();
        let mut out = format::paint(MARK, Tone::Accent, options);
        out.push(' ');
        if self.text.lines.is_empty() {
            out.push_str(&format::paint(&self.text.name, Tone::Code, options));
            out.push('\n');
            return out;
        }
        for (at, line) in self.text.lines.iter().enumerate() {
            let rows = match columns {
                Some((width, method)) => {
                    let first = width.saturating_sub(column).max(1);
                    let rest = width.saturating_sub(column + GOES_ON).max(1);
                    format::wrap_command(line, first, rest, method)
                }
                None => vec![(line.clone(), false)],
            };
            for (row, (text, goes_on)) in rows.iter().enumerate() {
                if at == 0 && row == 0 {
                    let first = format!("{} {text}", self.text.name);
                    out.push_str(&format::paint(&first, Tone::Code, options));
                } else {
                    let indent = if row > 0 { column + GOES_ON } else { column };
                    out.push_str(&" ".repeat(indent));
                    out.push_str(&format::paint(text, Tone::Code, options));
                }
                if *goes_on {
                    out.push_str(&format::paint(WRAP_MARK, Tone::Dim, options));
                }
                out.push('\n');
            }
        }
        out
    }

    /// The result row of the call that ended at `end` on the daemon's clock as
    /// `outcome` says, with its newline.
    pub(crate) fn result(
        &self,
        end: Option<Timestamp>,
        outcome: Outcome<'_>,
        options: &RenderOptions,
    ) -> String {
        let (mut text, tone) = outcome.result();
        let took = self.since.zip(end).map(|(since, end)| since_then(since, end));
        if let Some(took) = took.filter(|took| *took >= SHOW_TIME && outcome.ran()) {
            text.push_str(if outcome == Outcome::Ran { " " } else { JOIN });
            text.push_str(&format::took(took));
        }
        let mut row = INDENT.to_owned();
        row.push_str(&format::paint(&text, tone, options));
        row.push('\n');
        row
    }
}

/// Muted rows of notes about a call under its result, such as a connection that the
/// proxy blocked.
pub(crate) fn notes(lines: &[String], options: &RenderOptions) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&format::paint(&format!("{INDENT}{line}"), Tone::Dim, options));
        out.push('\n');
    }
    out
}

/// The lines of output after the bar, muted and cut to the width on a terminal;
/// `prompt` paints the last one in the `warning` role, as the prompt of a question.
pub(crate) fn tail(lines: &[String], prompt: bool, options: &RenderOptions) -> String {
    let method = options.width_method();
    let room = usize::from(options.width()).saturating_sub(text_width(BAR, method));
    let mut out = String::new();
    for (at, line) in lines.iter().enumerate() {
        // NOTE: elsewhere than on a terminal no screen sets a width, so nothing is cut.
        let line =
            if options.is_terminal() { format::cut(line, room, method) } else { line.clone() };
        let last = at + 1 == lines.len();
        if prompt && last {
            out.push_str(&format::paint(BAR, Tone::Dim, options));
            out.push_str(&format::paint(&line, Tone::Attention, options));
        } else {
            out.push_str(&format::paint(&format!("{BAR}{line}"), Tone::Dim, options));
        }
        out.push('\n');
    }
    out
}

/// The last [`TAIL_LINES`] lines of `output` with text in them, without the spaces
/// around them and safe to print.
pub(crate) fn last_lines(output: &str) -> Vec<String> {
    let mut lines: Vec<String> = output
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(TAIL_LINES)
        .map(format::one_line)
        .collect();
    lines.reverse();
    lines
}

/// The last lines of a tool's `output` as the model got it, for a call whose output
/// showed no line while it ran: the notes that efr adds at its end in brackets, such
/// as `[exit code 2, cwd /home/me]`, are left out.
pub(crate) fn output_lines(output: &str) -> Vec<String> {
    let mut lines: Vec<&str> = output.lines().collect();
    while lines.last().is_some_and(|line| {
        let line = line.trim();
        line.is_empty() || (line.starts_with('[') && line.ends_with(']'))
    }) {
        lines.pop();
    }
    last_lines(&lines.join("\n"))
}

#[cfg(test)]
mod tests;
