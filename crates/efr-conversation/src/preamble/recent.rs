//! The terminal's recent commands as the preamble shows them: after the last command,
//! oldest first, each with its secrets redacted and cut as the last command is.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::RecentCommand;
use jiff::Timestamp;

use super::{cut, secrets};

/// The most characters of all the entries together. The oldest entries go first, so
/// the commands nearest to the prompt stay.
pub(crate) const LIST_MAX_CHARS: usize = 2_000;

/// The newest `count` of `commands`, which are oldest first.
pub(crate) fn newest(commands: &[RecentCommand], count: usize) -> Vec<RecentCommand> {
    commands[commands.len().saturating_sub(count)..].to_vec()
}

/// The block that lists `commands` (oldest first) for a turn in `cwd` at `now`, or an
/// empty string when no entry remains. The newest entry goes when it is the
/// `last_command`, which its own line shows already.
pub(super) fn render(
    commands: &[RecentCommand],
    last_command: Option<&str>,
    cwd: &Path,
    now: Timestamp,
) -> String {
    let mut commands = commands;
    if let (Some((newest, older)), Some(last)) = (commands.split_last(), last_command)
        && newest.command.trim_end() == last.trim_end()
    {
        commands = older;
    }
    let mut kept = Vec::new();
    let mut total = 0;
    for command in commands.iter().rev() {
        if command.command.trim().is_empty() {
            continue;
        }
        let entry = entry(command, cwd, now);
        total += entry.chars().count();
        if total > LIST_MAX_CHARS {
            break;
        }
        kept.push(entry);
    }
    if kept.is_empty() {
        return String::new();
    }
    let mut text = String::from("The user's recent commands in this terminal, oldest first:\n");
    for entry in kept.iter().rev() {
        text.push_str(entry);
    }
    text
}

/// One entry of the list, with its line end.
fn entry(command: &RecentCommand, cwd: &Path, now: Timestamp) -> String {
    let line = cut(&secrets::redact(command.command.trim_end()));
    let mut facts = Vec::new();
    if let Some(status) = command.status {
        facts.push(format!("status {status}"));
    }
    if let Some(ms) = command.duration_ms.filter(|ms| *ms >= 1_000) {
        facts.push(format!("took {}", rough(ms / 1_000)));
    }
    if let Some(ended) = command.ended_at {
        // NOTE: a clock that differs between the shell and efrd can put the end after
        // now; that reads as just now.
        let secs = u64::try_from(now.as_second().saturating_sub(ended.as_second())).unwrap_or(0);
        if secs < 60 {
            facts.push("ended just now".to_owned());
        } else {
            facts.push(format!("ended {} ago", rough(secs)));
        }
    }
    if let Some(dir) = command.cwd.as_deref().filter(|dir| *dir != cwd) {
        facts.push(format!("in {}", dir.display()));
    }
    let facts = facts.join(", ");
    let mut text = String::new();
    // NOTE: writing to a String cannot fail, so the results are ignored.
    if line.contains('\n') || line.contains('`') {
        let facts = if facts.is_empty() { String::new() } else { format!(" ({facts})") };
        let _ = write!(text, "- a command{facts}:\n```\n{line}\n```\n");
    } else if facts.is_empty() {
        let _ = writeln!(text, "- `{line}`");
    } else {
        let _ = writeln!(text, "- `{line}` ({facts})");
    }
    text
}

/// `secs` in rough words: `40 s`, `3 min`, `2 h` or `5 d`.
fn rough(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3_600 => format!("{} min", secs / 60),
        3_600..86_400 => format!("{} h", secs / 3_600),
        _ => format!("{} d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests;
