//! The line before a reply that names where a prompt went (DV4, DV52): the switch of
//! the terminal's conversation, such as `↪ back to "fix the tests" in app (2 h ago)`,
//! or, when the conversation did not change, the scope of the prompt: `app`, or
//! `○ no project · ~/notes`. The view writes it muted, as a note. A prompt outside
//! projects in a git repository gets a second muted line at most once a day (DV53):
//! `○ ~/p/tool is a git repo but not a project: writes there ask you. efr project add
//! ~/p/tool registers it.` The status row of a running turn ends with the scope, such
//! as `app` or `○ ~/notes`. `efr history` shows a `terminal_attached` event as `picked
//! up in /dev/pts/7`.
//!
//! The glyphs `↪` and `○` come from the user's design. efr-cli has no ASCII path for
//! its other glyphs (`✓`, `·`, the spinner) either, so these have none.

use std::path::Path;

use efr_protocol::{AttachHow, NewReason, RepoHint, RouteLabel, RouteScope, Routed, Switch};
use jiff::Timestamp;

use super::one_line;
use super::sandbox::tilde;

/// The mark of a switch.
const SWITCH: &str = "↪";

/// The mark of a scope outside registered projects.
const OUTSIDE: &str = "○";

/// The line before the reply for `route`, with the settings `note` of the prompt
/// joined after ` · `, so a turn starts with one line.
pub(crate) fn line(
    route: &Routed,
    note: Option<&str>,
    now: Timestamp,
    home: Option<&Path>,
) -> String {
    let head = match &route.switch {
        Some(switch) => switch_line(switch, &route.scope, now, home),
        None => head_line(&route.scope, home),
    };
    match note {
        Some(note) => format!("{head} · {note}"),
        None => head,
    }
}

/// The scope of a prompt: the project's name, or `○ no project · <pwd>`.
pub(crate) fn head_line(scope: &RouteLabel, home: Option<&Path>) -> String {
    match (&scope.scope, &scope.name) {
        (RouteScope::Project(_), Some(name)) => one_line(name),
        (RouteScope::Project(_), None) => format!("removed project · {}", tilde(&scope.dir, home)),
        _ => format!("{OUTSIDE} no project · {}", tilde(&scope.dir, home)),
    }
}

/// How the terminal's conversation changed, for a prompt in `scope`.
pub(crate) fn switch_line(
    switch: &Switch,
    scope: &RouteLabel,
    now: Timestamp,
    home: Option<&Path>,
) -> String {
    match switch {
        Switch::Back { title, updated_at } => format!(
            "{SWITCH} back to {} {} ({})",
            titled(title.as_deref()),
            in_scope(scope),
            age(*updated_at, now)
        ),
        Switch::PickedUp { title, updated_at } => format!(
            "{SWITCH} picked up {} {} ({}); ,new starts fresh",
            titled(title.as_deref()),
            in_scope(scope),
            age(*updated_at, now)
        ),
        Switch::New { from, reason } => {
            let why = match (reason, from) {
                (NewReason::Scope, Some(from)) => {
                    format!(" (from {})", from_label(from, scope, home))
                }
                (NewReason::Idle { hours }, _) => {
                    format!(" (the last one was idle for {hours} h)")
                }
                (NewReason::Moved { tty, title }, _) => {
                    format!(" ({} moved to {})", titled(title.as_deref()), one_line(tty))
                }
                _ => String::new(),
            };
            format!("{SWITCH} new conversation {}{why}", for_scope(scope))
        }
        _ => format!("{SWITCH} {}", head_line(scope, home)),
    }
}

/// `in app`, or `outside projects`.
fn in_scope(scope: &RouteLabel) -> String {
    match (&scope.scope, &scope.name) {
        (RouteScope::Project(_), Some(name)) => format!("in {}", one_line(name)),
        (RouteScope::Project(_), None) => "in a removed project".to_owned(),
        _ => "outside projects".to_owned(),
    }
}

/// `for app`, or `outside projects`.
fn for_scope(scope: &RouteLabel) -> String {
    match (&scope.scope, &scope.name) {
        (RouteScope::Project(_), Some(name)) => format!("for {}", one_line(name)),
        (RouteScope::Project(_), None) => "for a removed project".to_owned(),
        _ => "outside projects".to_owned(),
    }
}

/// Where the terminal was before: the project's name, or the last directory of its
/// conversation outside projects. That directory can be inside the project of the new
/// conversation `to` (the prompts there went outside projects while
/// `conversation.follow_project` was off); then `outside projects` names it.
fn from_label(label: &RouteLabel, to: &RouteLabel, home: Option<&Path>) -> String {
    match (&label.scope, &label.name) {
        (RouteScope::Project(_), Some(name)) => one_line(name),
        (_, _) if label.dir.as_os_str().is_empty() => "no project".to_owned(),
        (RouteScope::Project(_), None) => tilde(&label.dir, home),
        _ if to.scope.project().is_some() && label.dir.starts_with(&to.dir) => {
            "outside projects".to_owned()
        }
        _ => tilde(&label.dir, home),
    }
}

/// The muted line of the hint in a git repository that is not a registered project
/// (DV53). The command in it is one that the user can paste: a path that a shell word
/// cannot hold as it is shows in single quotes, in full.
pub(crate) fn hint_line(hint: &RepoHint, home: Option<&Path>) -> String {
    let shown = tilde(&hint.root, home);
    format!(
        "{OUTSIDE} {shown} is a git repo but not a project: writes there ask you. efr project add {} registers it.",
        shell_word(&hint.root, &shown)
    )
}

/// `shown` when a shell reads it as one word for the path `path` (a `~` at its start
/// included), else `path` in single quotes.
fn shell_word(path: &Path, shown: &str) -> String {
    let plain = shown.chars().enumerate().all(|(at, c)| {
        c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c) || (c == '~' && at == 0)
    });
    if plain {
        return shown.to_owned();
    }
    let full = one_line(&path.display().to_string());
    format!("'{}'", full.replace('\'', r"'\''"))
}

/// The scope at the end of the status row of a running turn: the project's name, or
/// `○` and the prompt's directory outside projects.
pub(crate) fn scope_mark(scope: &RouteLabel, home: Option<&Path>) -> String {
    match (&scope.scope, &scope.name) {
        (RouteScope::Project(_), Some(name)) => one_line(name),
        (RouteScope::Project(_), None) => "removed project".to_owned(),
        _ => format!("{OUTSIDE} {}", tilde(&scope.dir, home)),
    }
}

/// A conversation's title in quotes, or what stands for a missing one.
fn titled(title: Option<&str>) -> String {
    match title {
        Some(title) => format!("\"{}\"", one_line(title)),
        None => "a conversation with no title".to_owned(),
    }
}

/// How long ago `then` was, in one coarse unit: `just now`, `25 min ago`, `2 h ago`,
/// `3 d ago`.
pub(crate) fn age(then: Timestamp, now: Timestamp) -> String {
    let seconds = now.duration_since(then).as_secs().max(0).unsigned_abs();
    match seconds {
        0..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", seconds / 60),
        3_600..86_400 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// The line of `efr history` for a `terminal_attached` event.
pub(crate) fn attached(tty: &str, how: AttachHow) -> String {
    let what = match how {
        AttachHow::Resumed => "resumed",
        AttachHow::PickedUp => "picked up",
        _ => "attached",
    };
    format!("{what} in {}", one_line(tty))
}

#[cfg(test)]
mod tests;
