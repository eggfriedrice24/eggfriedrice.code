//! The lines of `,resume` and `efr resume` (DV5): the numbered list of the recent
//! conversations, plain lines inline (never a full-screen picker, efr is not a TUI),
//! and the one muted line after a resume.
//!
//! A row is the number, the title, the scope (the project's name, `no project` or
//! `removed project`), how long ago the conversation was active, its prompt turns and
//! a muted mark of the terminal that holds it. On a terminal the titles are cut so a
//! row fits the width; in a pipe nothing is cut and nothing is coloured.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{ConversationsRecentResult, RecentConversation, RouteLabel, RouteScope};
use efr_render::{RenderOptions, WidthMethod, text_width};
use jiff::Timestamp;

use super::route::age;
use super::sandbox::tilde;
use super::{Tone, columns, cut, one_line, paint};

/// The mark of a resume, as of every switch of conversation.
const SWITCH: &str = "↪";

/// What stands for a conversation without a title.
const NO_TITLE: &str = "(no title)";

/// The narrowest a title gets on a narrow terminal.
pub(crate) const MIN_TITLE: usize = 12;

/// The list of `conversations.recent`, then the line that says how to resume one:
/// `,resume N` from the zsh plugin (`plugin`), else `efr resume N`.
pub(crate) fn list(
    recent: &ConversationsRecentResult,
    now: Timestamp,
    plugin: bool,
    options: &RenderOptions,
) -> String {
    if recent.conversations.is_empty() {
        return "no conversations yet\n".to_owned();
    }
    let method = columns(options).map_or(WidthMethod::CodePoint, |(_, method)| method);
    let rows: Vec<Row> = recent.conversations.iter().map(|row| Row::of(row, now)).collect();
    let width = |pick: fn(&Row) -> &str| {
        rows.iter().map(|row| text_width(pick(row), method)).max().unwrap_or(0)
    };
    let (scope_w, age_w, turns_w) =
        (width(|row| &row.scope), width(|row| &row.age), width(|row| &row.turns));
    let holder_w = width(|row| &row.holder);
    // The number, the gaps between the columns, and the holder after three blanks.
    let fixed = 2 + 2 + 2 + scope_w + 2 + age_w + 2 + turns_w + holder_w.min(1) * (3 + holder_w);
    let title_w = match columns(options) {
        Some((cols, _)) => width(|row| &row.title).min(cols.saturating_sub(fixed).max(MIN_TITLE)),
        None => width(|row| &row.title),
    };
    let mut out = String::new();
    for (index, row) in rows.iter().enumerate() {
        let title = match columns(options) {
            Some(_) => cut(&row.title, title_w, method),
            None => row.title.clone(),
        };
        let mut line = format!(
            "{:>2}  {}  {}  {}  {}",
            index + 1,
            pad(&title, title_w, method),
            pad(&row.scope, scope_w, method),
            pad(&row.age, age_w, method),
            pad_left(&row.turns, turns_w, method),
        );
        if !row.holder.is_empty() {
            line.push_str("   ");
            line.push_str(&paint(&row.holder, Tone::Dim, options));
        }
        let _ = writeln!(out, "{}", line.trim_end());
    }
    let how = if plugin { ",resume N" } else { "efr resume N" };
    let _ = writeln!(out, "{}", paint(&format!("{how} continues one here"), Tone::Dim, options));
    out
}

/// The cells of one row, before the columns are padded. `efr search` lists its rows
/// with the same cells.
pub(crate) struct Row {
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) age: String,
    pub(crate) turns: String,
    pub(crate) holder: String,
}

impl Row {
    pub(crate) fn of(row: &RecentConversation, now: Timestamp) -> Row {
        let title = row.summary.title.as_deref().map_or_else(|| NO_TITLE.to_owned(), one_line);
        let scope = match (&row.route, &row.project) {
            (RouteScope::Project(_), Some(name)) => one_line(name),
            (RouteScope::Project(_), None) => "removed project".to_owned(),
            _ => "no project".to_owned(),
        };
        let turns = match row.turns {
            1 => "1 turn".to_owned(),
            count => format!("{count} turns"),
        };
        let holder = match (&row.open_in, row.here) {
            (_, true) => "this terminal".to_owned(),
            (Some(tty), false) => format!("open in {}", one_line(tty)),
            (None, false) => String::new(),
        };
        Row { title, scope, age: age(row.summary.updated_at, now), turns, holder }
    }
}

/// `text` with blanks after it to `width` columns.
pub(crate) fn pad(text: &str, width: usize, method: WidthMethod) -> String {
    let blanks = width.saturating_sub(text_width(text, method));
    format!("{text}{}", " ".repeat(blanks))
}

/// `text` with blanks before it to `width` columns.
pub(crate) fn pad_left(text: &str, width: usize, method: WidthMethod) -> String {
    let blanks = width.saturating_sub(text_width(text, method));
    format!("{}{text}", " ".repeat(blanks))
}

/// What a resume did, for the muted line after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resumed<'a> {
    pub(crate) title: Option<&'a str>,
    pub(crate) scope: &'a RouteLabel,
    pub(crate) updated_at: Timestamp,
    /// The terminal held the conversation already.
    pub(crate) here: bool,
    /// A prompt from the user's directory continues it.
    pub(crate) continues_here: bool,
}

/// The line after a resume: `↪ resumed "fix the tests" (app, 2 h ago)`, with where it
/// continues when the user's directory is in another scope, or `↪ "fix the tests" is
/// this terminal's app conversation` when the terminal held it already.
pub(crate) fn resumed_line(resumed: &Resumed<'_>, now: Timestamp, home: Option<&Path>) -> String {
    let title = match resumed.title {
        Some(title) => format!("\"{}\"", one_line(title)),
        None => "the conversation with no title".to_owned(),
    };
    let mut line = if resumed.here {
        match (&resumed.scope.scope, &resumed.scope.name) {
            (RouteScope::Project(_), Some(name)) => {
                format!("{SWITCH} {title} is this terminal's {} conversation", one_line(name))
            }
            (RouteScope::Project(_), None) => {
                format!("{SWITCH} {title} is this terminal's conversation of a removed project")
            }
            _ => format!("{SWITCH} {title} is this terminal's conversation outside projects"),
        }
    } else {
        let scope = match (&resumed.scope.scope, &resumed.scope.name) {
            (RouteScope::Project(_), Some(name)) => one_line(name),
            (RouteScope::Project(_), None) => "removed project".to_owned(),
            _ => "no project".to_owned(),
        };
        format!("{SWITCH} resumed {title} ({scope}, {})", age(resumed.updated_at, now))
    };
    if !resumed.continues_here {
        let place = match &resumed.scope.scope {
            RouteScope::Project(_) => format!("in {}", tilde(&resumed.scope.dir, home)),
            _ => "outside projects".to_owned(),
        };
        let _ = write!(line, "; it continues when you prompt {place}");
    }
    line
}

/// The question when another open terminal holds the conversation.
pub(crate) fn held_question(title: Option<&str>, tty: &str) -> String {
    let title = match title {
        Some(title) => format!("\"{}\"", one_line(title)),
        None => "the conversation".to_owned(),
    };
    format!("{title} is open in {}. Move it here? [y/N]", one_line(tty))
}

#[cfg(test)]
mod tests;
