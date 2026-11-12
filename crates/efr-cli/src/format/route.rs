//! The event lines at the start of a reply (DV4, DV52, `DECISIONS.md` "The frame of a
//! turn"): a reply has no first line, and a line comes only when something changed.
//! `↪` names a change of the terminal's conversation, such as `↪ back to "fix the
//! tests" in app (2 h ago)`; `⇄` names a switch of model, provider or account, such as
//! `⇄ b/gpt-5.5 medium  was work/claude-opus-5-5 high`. When both change, `↪` comes
//! first. Each line is pieces with their tones (TM-Legend 4, 5; TM-Scope 19): the mark
//! in `accent`, the sentence in `text`, the age and what follows it `muted`, a command
//! such as `,new` in `code`.
//!
//! The place outside projects is home: the mark `⌂` and the words "at home", "from
//! home". A prompt at home in a git repository gets the repo hint at most once a day
//! (DV53): `⌂ ~/p/tool is a git repo but not a project: writes there ask you.` muted,
//! then `efr project add ~/p/tool` in `code`. The status row of a running turn ends with
//! the scope, such as `app` or `⌂ ~/notes`. `efr history` shows a `terminal_attached`
//! event as `picked up in /dev/pts/7`.
//!
//! Every glyph comes from `efr_render::Glyphs`, which has an ASCII form of each.

use std::path::Path;

use efr_protocol::{AttachHow, NewReason, RepoHint, RouteLabel, RouteScope, Switch, SwitchedModel};
use efr_render::Glyphs;
use jiff::Timestamp;

use super::context::count;
use super::sandbox::tilde;
use super::{Tone, one_line};

/// A line of efr's own as pieces of text, each with its tone.
pub(crate) type Pieces = Vec<(String, Tone)>;

/// The `↪` line of a change of the terminal's conversation, for a prompt in `scope`
/// (TM-Scope 19): the mark, the sentence, then the age or the reason in parentheses.
pub(crate) fn switch_line(
    switch: &Switch,
    scope: &RouteLabel,
    now: Timestamp,
    home: Option<&Path>,
    glyphs: &Glyphs,
) -> Pieces {
    let (sentence, rest) = match switch {
        Switch::Back { title, updated_at } => (
            format!("back to {} {}", titled(title.as_deref()), in_scope(scope)),
            format!("({})", age(*updated_at, now)),
        ),
        Switch::PickedUp { title, updated_at } => {
            let mut line = event_line(
                glyphs.conversation,
                &format!("picked up {} {}", titled(title.as_deref()), in_scope(scope)),
                &format!("({}) {} ", age(*updated_at, now), glyphs.separator),
            );
            line.push((",new".to_owned(), Tone::Code));
            line.push((" starts fresh".to_owned(), Tone::Dim));
            return line;
        }
        Switch::New { from, reason } => {
            let why = match (reason, from) {
                (NewReason::Scope, Some(from)) => format!("(from {})", from_label(from, home)),
                (NewReason::Idle { hours }, _) => {
                    format!("(the last one was idle for {hours} h)")
                }
                (NewReason::Moved { tty, title }, _) => {
                    format!("({} moved to {})", titled(title.as_deref()), one_line(tty))
                }
                _ => String::new(),
            };
            (format!("new conversation {}", for_scope(scope)), why)
        }
        // NOTE: a kind of switch from a newer daemon: the line still says that the
        // conversation changed, with no reason that this efr cannot read.
        _ => (format!("another conversation {}", in_scope(scope)), String::new()),
    };
    event_line(glyphs.conversation, &sentence, &rest)
}

/// An event line: `mark` in `accent`, then ` sentence` in `text`, then ` rest` muted.
fn event_line(mark: &str, sentence: &str, rest: &str) -> Pieces {
    let mut line = vec![(mark.to_owned(), Tone::Accent), (format!(" {sentence}"), Tone::Text)];
    if !rest.is_empty() {
        line.push((format!(" {rest}"), Tone::Dim));
    }
    line
}

/// The `↪` line of a turn that a job notice started, not the user (spec 1.4): the mark
/// in `accent` and the sentence muted, as a notice.
pub(crate) fn wake_line(glyphs: &Glyphs) -> Pieces {
    vec![
        (glyphs.conversation.to_owned(), Tone::Accent),
        (format!(" {}", super::jobs::WAKE_WORDS), Tone::Dim),
    ]
}

/// `in app`, or `at home`.
fn in_scope(scope: &RouteLabel) -> String {
    match (&scope.scope, &scope.name) {
        (RouteScope::Project(_), Some(name)) => format!("in {}", one_line(name)),
        (RouteScope::Project(_), None) => "in a removed project".to_owned(),
        _ => "at home".to_owned(),
    }
}

/// `for app`, or `at home`.
fn for_scope(scope: &RouteLabel) -> String {
    match (&scope.scope, &scope.name) {
        (RouteScope::Project(_), Some(name)) => format!("for {}", one_line(name)),
        (RouteScope::Project(_), None) => "for a removed project".to_owned(),
        _ => "at home".to_owned(),
    }
}

/// Where the terminal was before: the project's name, the root of a project that is no
/// longer registered, or `home`.
fn from_label(label: &RouteLabel, home: Option<&Path>) -> String {
    match (&label.scope, &label.name) {
        (RouteScope::Project(_), Some(name)) => one_line(name),
        (RouteScope::Project(_), None) if !label.dir.as_os_str().is_empty() => {
            tilde(&label.dir, home)
        }
        _ => "home".to_owned(),
    }
}

/// The line of the hint at home in a git repository that is not a registered project
/// (DV53, TM-Scope 19): the sentence muted, then the command that registers it in
/// `code`. The command is one that the user can paste: a path that a shell word cannot
/// hold as it is shows in single quotes, in full.
pub(crate) fn hint_line(hint: &RepoHint, home: Option<&Path>, glyphs: &Glyphs) -> Pieces {
    let shown = tilde(&hint.root, home);
    vec![
        (
            format!(
                "{} {shown} is a git repo but not a project: writes there ask you.",
                glyphs.home
            ),
            Tone::Dim,
        ),
        (" ".to_owned(), Tone::Plain),
        (format!("efr project add {}", shell_word(&hint.root, &shown)), Tone::Code),
    ]
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
/// the home mark and the prompt's directory.
pub(crate) fn scope_mark(scope: &RouteLabel, home: Option<&Path>, glyphs: &Glyphs) -> String {
    match (&scope.scope, &scope.name) {
        (RouteScope::Project(_), Some(name)) => one_line(name),
        (RouteScope::Project(_), None) => "removed project".to_owned(),
        _ => format!("{} {}", glyphs.home, tilde(&scope.dir, home)),
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

/// The line of a switch of model, provider or account (TM-Legend 5, TM-Endings 5 to 7,
/// TM-Scope 19): `⇄ b/gpt-5.5 medium  was work/claude-opus-5-5 high`, the mark in
/// `accent`, the new side in `text`, the rest `muted`. `from` and `to` are the sides of
/// `model_switched`. Then, after the separator, `context compacted 86k → 6k` when a
/// compaction of the turn came before anything else (`compacted`: the tokens before and
/// after), else `cache starts cold` when only the provider or the account changed: the
/// prompt cache of one provider does not serve another.
pub(crate) fn model_line(
    from: &SwitchedModel,
    to: &SwitchedModel,
    compacted: Option<(u64, u64)>,
    glyphs: &Glyphs,
) -> Pieces {
    let mut rest = format!("was {}", side(from));
    if let Some((before, after)) = compacted {
        rest.push_str(&format!(
            " {} context compacted {} {} {}",
            glyphs.separator,
            count(before),
            glyphs.arrow,
            count(after)
        ));
    } else if from.model == to.model {
        rest.push_str(&format!(" {} cache starts cold", glyphs.separator));
    }
    vec![
        (glyphs.switch.to_owned(), Tone::Accent),
        (format!(" {}  ", side(to)), Tone::Text),
        (rest, Tone::Dim),
    ]
}

/// One side of a switch as `,model` takes it: `work/claude-opus-5-5 high`, the account
/// of the provider id (`anthropic-api.work` names `work`), the model and the effort.
/// A provider id with no account gives the model alone, and a turn that sent no effort
/// gives no effort.
fn side(model: &SwitchedModel) -> String {
    let account = model.provider.as_deref().and_then(|provider| provider.split_once('.'));
    let mut text = match account {
        Some((_, account)) if !account.is_empty() => {
            format!("{}/{}", one_line(account), one_line(&model.model))
        }
        _ => one_line(&model.model),
    };
    if let Some(effort) = model.effort.as_deref().filter(|effort| !effort.is_empty()) {
        text.push(' ');
        text.push_str(&one_line(effort));
    }
    text
}

#[cfg(test)]
mod tests;
