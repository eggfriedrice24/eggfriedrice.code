//! A question as a card: a title after `? `, then rows after a thin bar, then the keys.
//!
//! ```text
//! ? allow outside the sandbox
//! │ cd ~/p/app && find target/debug/build \
//! │     -path '*ghostty*' -type f
//! │ runs with your full rights: files, secrets, network
//! │ programs: cd, find
//! │ y allow · n deny
//! ```
//!
//! The title and what needs the user's care are in the `warning` role, the bar and the
//! secondary facts are muted, and what runs is in the `code` role. Every line of a
//! command shows, each on its own, and nothing of what runs is cut: a line wider than
//! the screen goes on in the next row. A cut after a space ends in a muted `\` and the
//! rows from there are indented; a space cuts only a row that is then at least half
//! full. Else a word is cut, with a muted `↩`, and goes on in the next row at the same
//! column, so the rows show no space that the command does not have. Text rows go on
//! in the next row, indented. When the output is not a terminal, nothing wraps and
//! nothing is painted.

use efr_render::{RenderOptions, render};

use super::{
    CommandRow, RowEnd, Tone, code_block, columns, lines, paint, wrap_command, wrap_spans,
};

/// The mark before a question's title.
const MARK: &str = "? ";

/// What starts each row of a card.
pub(crate) const BAR: &str = "\u{2502} ";

/// How far a row of a command that goes on after a space is indented after the bar.
const COMMAND_INDENT: &str = "    ";

/// How far a row of text that goes on is indented after the bar.
const TEXT_INDENT: &str = "  ";

/// The keys of an approval.
pub(crate) const ALLOW_KEYS: &[(&str, &str)] = &[("y", "allow"), ("n", "deny")];

/// The keys of the quarantine question.
pub(crate) const KEEP_KEYS: &[(&str, &str)] = &[("y", "keep"), ("n", "leave in quarantine")];

/// The row of a question that another client must answer.
const WAITING: &str = "waiting for another client to answer";

/// One row of a card, before it is cut into the rows of a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    /// One line of a command, safe to print: what runs, in the `code` role.
    Command(String),
    /// Text in pieces with their tone, safe to print.
    Text(Vec<(String, Tone)>),
    /// A diff preview, as the daemon sent it.
    Diff(String),
}

impl Row {
    /// A row of one piece of text in `tone`.
    pub(crate) fn text(text: impl Into<String>, tone: Tone) -> Row {
        Row::Text(vec![(text.into(), tone)])
    }
}

/// What the last row of a card says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Footer {
    /// The keys that answer it here.
    Keys(&'static [(&'static str, &'static str)]),
    /// Another client must answer it.
    Waiting,
}

/// A question: its title, safe to print, and its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Card {
    pub(crate) title: String,
    pub(crate) rows: Vec<Row>,
}

impl Card {
    /// The card as lines, each with its newline, the last row `footer` when there is
    /// one: cut into rows of the screen's width on a terminal, as they are elsewhere.
    pub(crate) fn render(&self, footer: Option<Footer>, options: &RenderOptions) -> String {
        let columns = columns(options);
        let mut out = String::new();
        let title = vec![(format!("{MARK}{}", self.title), Tone::Attention)];
        for (at, row) in spans(&title, columns, 0, TEXT_INDENT.len()).into_iter().enumerate() {
            if at > 0 {
                out.push_str(TEXT_INDENT);
            }
            push_pieces(&mut out, &row, options);
            out.push('\n');
        }
        for row in &self.rows {
            push_row(&mut out, row, options);
        }
        if let Some(footer) = footer {
            out.push_str(&footer_row(footer, options));
        }
        out
    }
}

/// The last row of a card, with its newline: the keys, or that another client must
/// answer.
pub(crate) fn footer_row(footer: Footer, options: &RenderOptions) -> String {
    let mut out = paint(BAR, Tone::Dim, options);
    match footer {
        Footer::Keys(keys) => out.push_str(&super::keys(keys, options)),
        Footer::Waiting => out.push_str(&paint(WAITING, Tone::Dim, options)),
    }
    out.push('\n');
    out
}

/// Adds the screen rows of `row`, each after the bar, with their newlines.
fn push_row(out: &mut String, row: &Row, options: &RenderOptions) {
    let columns = columns(options);
    let bar = paint(BAR, Tone::Dim, options);
    let bar_width = BAR.chars().count();
    match row {
        Row::Command(line) => {
            let rows = match columns {
                Some((width, method)) => {
                    let room = width.saturating_sub(bar_width).max(1);
                    wrap_command(line, room, COMMAND_INDENT.len(), method)
                }
                None => {
                    vec![CommandRow { text: line.clone(), indented: false, end: RowEnd::Last }]
                }
            };
            for row in &rows {
                out.push_str(&bar);
                if row.indented {
                    out.push_str(COMMAND_INDENT);
                }
                out.push_str(&paint(&row.text, Tone::Code, options));
                if let Some(mark) = row.mark() {
                    out.push_str(&paint(mark, Tone::Dim, options));
                }
                out.push('\n');
            }
        }
        Row::Text(pieces) => {
            for (at, row) in
                spans(pieces, columns, bar_width, TEXT_INDENT.len()).into_iter().enumerate()
            {
                out.push_str(&bar);
                if at > 0 {
                    out.push_str(TEXT_INDENT);
                }
                push_pieces(out, &row, options);
                out.push('\n');
            }
        }
        Row::Diff(diff) => {
            let shown = if options.is_terminal() {
                let inner = options.width().saturating_sub(2).max(10);
                render(&code_block("diff", diff), &options.clone().with_width(inner))
            } else {
                lines(diff).into_owned()
            };
            for line in shown.lines() {
                out.push_str(&bar);
                out.push_str(line);
                out.push('\n');
            }
        }
    }
}

/// `pieces` cut into rows after `before` columns, with `indent` more on each row after
/// the first; one row when nothing wraps.
fn spans(
    pieces: &[(String, Tone)],
    columns: Option<(usize, efr_render::WidthMethod)>,
    before: usize,
    indent: usize,
) -> Vec<Vec<(String, Tone)>> {
    match columns {
        Some((width, method)) => {
            let first = width.saturating_sub(before).max(1);
            let rest = first.saturating_sub(indent).max(1);
            wrap_spans(pieces, first, rest, method)
        }
        None => vec![pieces.to_vec()],
    }
}

/// The card of an approval without an exit: `summary` is the daemon's, `tool` the
/// call's tool and `command` the command of its `tool_call_started`, when they are
/// known.
///
/// A summary that quotes exactly the call's command (`run "cd src\nls"`) shows each
/// line of it on its own; what the summary says besides follows after `why:`. Any
/// other summary shows as one row. The line of the parts of a command line that ask
/// follows after `asks for:`.
pub(crate) fn approval(summary: &str, tool: Option<&str>, command: Option<&str>) -> Card {
    let (first, asking) = super::split_summary(summary);
    let title = match tool {
        Some("shell") => "allow this command",
        Some("write_file") => "allow this write",
        Some("read_file") => "allow this read",
        Some("settings") => "allow this change of the settings",
        _ => "allow this call",
    };
    let subjects = tool.and_then(|tool| first.strip_prefix(&format!("{tool}: ")));
    let mut rows = Vec::new();
    let quoted = command.zip(subjects).and_then(|(command, subjects)| {
        let (before, after) = super::around_quote(subjects, &format!("run {command:?}"))?;
        Some((command, before, after))
    });
    match quoted {
        Some((command, before, after)) => {
            rows.extend(command_rows(command));
            let rest: Vec<&str> = [before.trim_end_matches("; ").trim(), after.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            if !rest.is_empty() {
                rows.push(Row::text(
                    format!("why: {}", super::one_line(&rest.join("; "))),
                    Tone::Dim,
                ));
            }
        }
        None => rows.push(Row::text(super::one_line(subjects.unwrap_or(first)), Tone::Plain)),
    }
    if let Some(asking) = asking {
        rows.push(Row::text(asking, Tone::Dim));
    }
    Card { title: title.to_owned(), rows }
}

/// A row for each line of `command`, safe to print. A carriage return at the end of a
/// line shows as its stand-in, because a shell does not read it as a space.
pub(crate) fn command_rows(command: &str) -> Vec<Row> {
    super::command_lines(command)
        .into_iter()
        .map(|line| Row::Command(super::command_line(line)))
        .collect()
}

fn push_pieces(out: &mut String, pieces: &[(String, Tone)], options: &RenderOptions) {
    for (text, tone) in pieces {
        out.push_str(&paint(text, *tone, options));
    }
}

#[cfg(test)]
mod tests;
