//! A tool call of the followed turn on a terminal: one line in the live zone while it
//! runs, and one committed line when it ends.
//!
//! While the call runs, its line carries the spinner, what the call does and, from
//! 1 s on, how long it has run, such as `⠹ $ cargo test -p app  12s`; the last
//! [`TAIL_LINES`] lines of its output with text in them follow, each after `  │ `. The
//! status row hides meanwhile, so the screen shows one sign of work. A call whose
//! approval waits shows nothing yet: the question is on the screen.
//!
//! When the call ends, one muted line is written once in place of the live lines:
//! `$ cargo test -p app  6.2s`, with `exit 101` in the `error` role when it failed and
//! `refused: <why>` in the `warning` role when efr refused it. The time shows for a
//! call of 1 s or more. A failed call keeps the last lines of its output below its
//! line; a call that went well keeps none.

use std::time::Duration;

use efr_protocol::CallId;
use efr_render::{RenderOptions, Role, text_width};
use jiff::Timestamp;

use crate::follow::since_then;
use crate::format::{self, CallLine, Tone};

/// The lines of output that a running call shows and a failed one keeps.
pub(crate) const TAIL_LINES: usize = 3;

/// The time from which a call's line says how long it ran.
const SHOW_TIME: Duration = Duration::from_secs(1);

/// What starts a line of a call's output.
pub(crate) const BAR: &str = "  \u{2502} ";

/// How a call ended, for its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome<'a> {
    /// It ran and went well, or its end says nothing more (a setup failure has a line
    /// of its own).
    Ran,
    /// Its command exited with this code.
    Exited { code: i32, contained: bool },
    /// The tool failed without an exit code.
    Failed { contained: bool },
    /// efr refused it before it ran, for this reason.
    Refused(&'a str),
}

impl Outcome<'_> {
    /// True when the call failed, so its last output lines stay on the screen.
    pub(crate) fn failed(self) -> bool {
        matches!(self, Outcome::Exited { .. } | Outcome::Failed { .. })
    }

    /// The pieces after the call, each with its tone.
    fn pieces(self) -> Vec<(String, Tone)> {
        let sandbox = |contained: bool| {
            if contained { vec![(" (sandbox)".to_owned(), Tone::Dim)] } else { Vec::new() }
        };
        match self {
            Outcome::Ran => Vec::new(),
            Outcome::Exited { code, contained } => {
                let mut pieces = vec![(format!("exit {code}"), Tone::Failure)];
                pieces.extend(sandbox(contained));
                pieces
            }
            Outcome::Failed { contained } => {
                let mut pieces = vec![("failed".to_owned(), Tone::Failure)];
                pieces.extend(sandbox(contained));
                pieces
            }
            Outcome::Refused(reason) => {
                vec![(format!("refused: {}", format::one_line(reason)), Tone::Attention)]
            }
        }
    }
}

/// A tool call of the followed turn, from its start to its end.
#[derive(Debug)]
pub(crate) struct Call {
    pub(crate) call_id: CallId,
    line: CallLine,
    /// The daemon's time when it began to run: its start, or the answer to its
    /// approval.
    since: Option<Timestamp>,
    /// The time of the first frame that showed it running.
    shown: Option<Timestamp>,
    /// Its approval waits for an answer, so it does not run yet.
    pub(crate) awaiting: bool,
}

impl Call {
    /// Call `call_id`, whose line is `line`, started at `since` on the daemon's clock.
    pub(crate) fn new(call_id: CallId, line: CallLine, since: Option<Timestamp>) -> Call {
        Call { call_id, line, since, shown: None, awaiting: false }
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

    /// The line of the running call at `now`, with `spinner` and its newline.
    pub(crate) fn running(&self, spinner: char, now: Timestamp, options: &RenderOptions) -> String {
        let elapsed = self.shown.map_or(Duration::ZERO, |shown| since_then(shown, now));
        let time = (elapsed >= SHOW_TIME).then(|| format::elapsed(elapsed));
        let mut suffix = Vec::new();
        if let Some(time) = time {
            suffix.push((time, Tone::Dim));
        }
        let mut row = options.paint(Role::Accent, &spinner.to_string());
        row.push(' ');
        row.push_str(&self.painted(&suffix, 2, options));
        row
    }

    /// The line of the call that ended at `end` on the daemon's clock as `outcome`
    /// says, with its newline.
    pub(crate) fn ended(
        &self,
        end: Option<Timestamp>,
        outcome: Outcome<'_>,
        options: &RenderOptions,
    ) -> String {
        let mut suffix = outcome.pieces();
        let took = self.since.zip(end).map(|(since, end)| since_then(since, end));
        if let Some(took) = took.filter(|took| *took >= SHOW_TIME)
            && !matches!(outcome, Outcome::Refused(_))
        {
            suffix.push((format::took(took), Tone::Dim));
        }
        self.painted(&suffix, 0, options)
    }

    /// The call's line, cut to leave room for `before` columns in front of it and
    /// `suffix`, each piece of which follows after two spaces.
    fn painted(&self, suffix: &[(String, Tone)], before: usize, options: &RenderOptions) -> String {
        let method = options.width_method();
        let mut reserve = before;
        for (piece, _) in suffix {
            reserve += 2 + text_width(piece, method);
        }
        let columns = Some((usize::from(options.width()), method));
        let mut line = format::paint(&self.line.fit(columns, reserve), Tone::Dim, options);
        for (at, (piece, tone)) in suffix.iter().enumerate() {
            // A piece that starts with a space joins the one before it.
            if !(at > 0 && piece.starts_with(' ')) {
                line.push_str("  ");
            }
            line.push_str(&format::paint(piece, *tone, options));
        }
        line.push('\n');
        line
    }
}

/// The lines of output after the bar, muted and cut to the width; `prompt` paints the
/// last one in the `warning` role, as the prompt of a question.
pub(crate) fn tail(lines: &[String], prompt: bool, options: &RenderOptions) -> String {
    let method = options.width_method();
    let room = usize::from(options.width()).saturating_sub(text_width(BAR, method));
    let mut out = String::new();
    for (at, line) in lines.iter().enumerate() {
        let line = format::cut(line, room, method);
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
