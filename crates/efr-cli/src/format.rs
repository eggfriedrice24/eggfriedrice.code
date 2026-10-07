//! Text that the CLI writes itself: its own styled lines, one-line summaries of tool
//! calls, and the `status` and `history` listings. Markdown goes through `efr-render`;
//! everything here is plain lines.
//!
//! Text that comes from the daemon or the model (titles, summaries, error messages,
//! prompts) goes through [`one_line`] or [`lines`] first, so an escape sequence in it
//! cannot drive the terminal.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{
    AdminConfigReloadResult, AdminStatusResult, ApprovalDecision, ConfigFileError,
    ConversationStatus, ConversationsListResult, EffectiveSettings, Origin,
};
use efr_render::{ColourMode, RenderOptions};
use efr_stdx::text::is_format;
use jiff::Timestamp;
use serde_json::Value;
use unicode_width::UnicodeWidthChar as _;

pub(crate) mod sandbox;

/// Keys of a tool's input that best describe a call in one line, in order of
/// preference.
const DETAIL_KEYS: &[&str] = &["command", "cmd", "path", "file", "url", "query"];

/// How one of the CLI's own lines looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Bold: the user's own prompt.
    Bold,
    /// Dim: notes about the turn.
    Dim,
    /// Bold yellow, or bold without colour: something waits for the user.
    Attention,
}

impl Tone {
    fn sgr(self, colour: ColourMode) -> &'static str {
        match (self, colour) {
            (Tone::Bold, _) | (Tone::Attention, ColourMode::None) => "1",
            (Tone::Dim, _) => "2",
            (Tone::Attention, _) => "1;33",
        }
    }
}

/// The kinds of blocks a reply or a transcript is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Block {
    /// The user's prompt.
    Prompt,
    /// An assistant message.
    Message,
    /// A dim note: a tool call, an answer, the end of a turn.
    Note,
    /// An approval request.
    Approval,
}

/// Separates blocks by a blank line, except consecutive notes, which read as one
/// group.
#[derive(Debug, Default)]
pub(crate) struct Spacing {
    last: Option<Block>,
}

impl Spacing {
    /// What to write before a block of `kind`.
    pub(crate) fn before(&mut self, kind: Block) -> &'static str {
        let separator = match (self.last, kind) {
            (None, _) | (Some(Block::Note), Block::Note) => "",
            _ => "\n",
        };
        self.last = Some(kind);
        separator
    }
}

/// `text` styled as `tone` when the output is a terminal, plain otherwise. The text is
/// written as it is: callers pass text that is already safe.
pub(crate) fn paint(text: &str, tone: Tone, options: &RenderOptions) -> String {
    if !options.is_terminal() || text.is_empty() {
        return text.to_owned();
    }
    format!("\x1b[{}m{text}\x1b[0m", tone.sgr(options.colour()))
}

/// `text` on one line and safe to print: newlines and tabs become spaces, other control
/// characters visible stand-ins.
pub(crate) fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            c => visible(c),
        })
        .collect()
}

/// What starts the line of an approval summary that names the parts of a command line
/// that ask, as the daemon writes it.
const ASKS_FOR: &str = "\nasks for: ";

/// An approval summary as one line for the question and, when the daemon named them,
/// the parts of a command line that ask, as a line `asks for: hostnamectl, ...`. Both
/// are safe to print.
///
/// NOTE: the daemon escapes every other newline of a summary, and names a part only
/// with letters, digits, spaces and `._/:@%+,-`. A line with anything else is not
/// split off, so a file name with a newline in an older summary cannot pass for it.
pub(crate) fn approval_summary(summary: &str) -> (String, Option<String>) {
    let (first, asking) = split_summary(summary);
    (one_line(first), asking)
}

/// The summary without the line of the parts that ask, and that line.
fn split_summary(summary: &str) -> (&str, Option<String>) {
    let named = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, ' ' | '.' | '_' | '/' | ':' | '@' | '%' | '+' | ',' | '-')
    };
    match summary.rsplit_once(ASKS_FOR) {
        Some((first, parts)) if !parts.is_empty() && parts.chars().all(named) => {
            (first, Some(format!("asks for: {parts}")))
        }
        _ => (summary, None),
    }
}

/// What an approval question shows of the daemon's summary, as [`approval_summary`],
/// but with the lines of a command of several lines each on its own: `call` is the
/// tool and the command of the call, from its `tool_call_started`.
///
/// NOTE: the daemon writes the line as a quoted Rust string (`run "cd src\nls"`). Only
/// a summary with exactly that quote of the call's command as one of its parts gets
/// its lines; what the summary says besides the quote follows on a line of its own
/// after `also:`. The daemon puts the quote first, but an older one put a path first,
/// and `efr history` shows its summaries too. Any other summary shows as before.
pub(crate) fn approval_heading(
    summary: &str,
    call: Option<(&str, &str)>,
) -> (Vec<String>, Option<String>) {
    let (first, asking) = split_summary(summary);
    if let Some((tool, command)) = call
        && command_lines(command).len() > 1
        && let Some(subjects) = first.strip_prefix(&format!("{tool}: "))
        && let Some((before, after)) = around_quote(subjects, &format!("run {command:?}"))
    {
        let mut lines = run_heading(tool, command);
        let rest: Vec<&str> = [before.trim_end_matches("; ").trim(), after.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        if !rest.is_empty() {
            lines.push(format!("also: {}", one_line(&rest.join("; "))));
        }
        return (lines, asking);
    }
    (vec![one_line(first)], asking)
}

/// What `subjects` says before and after `quote`, when `quote` is one of its parts:
/// at the start or after `; `, and at the end or before `;`.
fn around_quote<'a>(subjects: &'a str, quote: &str) -> Option<(&'a str, &'a str)> {
    subjects.match_indices(quote).find_map(|(at, _)| {
        let before = subjects.get(..at)?;
        let after = subjects.get(at + quote.len()..)?;
        let starts = before.is_empty() || before.ends_with("; ");
        let ends = after.is_empty() || after.starts_with(';');
        (starts && ends).then(|| (before, after.trim_start_matches(';')))
    })
}

/// The command of a shell call's input, if it has one.
pub(crate) fn command_of(input: &Value) -> Option<&str> {
    input.get("command").and_then(Value::as_str)
}

/// `text` safe to print as lines: newlines stay, tabs become a space, other control
/// characters and format characters visible stand-ins.
pub(crate) fn lines(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| (c.is_control() && c != '\n') || is_format(c)) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|c| match c {
                '\n' => '\n',
                '\t' => ' ',
                c => visible(c),
            })
            .collect(),
    )
}

/// A control character's visible stand-in, the same mapping `efr-render` uses, and
/// `U+FFFD` for a format character, which draws nothing or turns the text around it.
/// A model's reply in markdown keeps format characters, for the scripts that need
/// them.
fn visible(c: char) -> char {
    match c {
        '\u{0}'..='\u{1f}' => char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'),
        '\u{7f}' => '\u{2421}',
        c if c.is_control() || is_format(c) => '\u{fffd}',
        c => c,
    }
}

/// `body` as a fenced markdown code block labelled `info`. The fence is longer than any
/// run of backticks in the body, so the body cannot close it early.
pub(crate) fn code_block(info: &str, body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    let newline = if body.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{info}\n{body}{newline}{fence}\n")
}

/// A tool call in one line, such as `shell: ls -la` or `read_file: /etc/hosts`.
///
/// A command of several lines shows its first line and how many lines follow, such as
/// `shell: cd src (and 2 more lines)`: lines joined by spaces would look like one
/// command with more arguments. With `columns`, the line is cut to fit that width, but
/// never the count of the lines that follow.
pub(crate) fn tool_call(tool: &str, input: &Value, columns: Option<usize>) -> String {
    let detail = match input {
        Value::Object(members) => DETAIL_KEYS
            .iter()
            .find_map(|key| members.get(*key).and_then(Value::as_str))
            .map_or_else(|| input.to_string(), str::to_owned),
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let tool = one_line(tool);
    let lines = command_lines(&detail);
    let (first, more) = match lines.split_first() {
        Some((first, rest)) => (command_line(first), rest.len()),
        None => (String::new(), 0),
    };
    let head = if first.is_empty() { tool } else { format!("{tool}: {first}") };
    if more == 0 {
        return head;
    }
    let unit = if more == 1 { "line" } else { "lines" };
    let tail = format!(" (and {more} more {unit})");
    match columns {
        Some(columns) => format!("{}{tail}", cut(&head, columns.saturating_sub(width(&tail)))),
        None => format!("{head}{tail}"),
    }
}

/// The lines of a command, split at each newline, without the blank lines at its start
/// and its end, which run nothing. A command of only blanks has none.
pub(crate) fn command_lines(command: &str) -> Vec<&str> {
    let lines: Vec<&str> = command.split('\n').collect();
    let blank = |line: &&str| line.trim().is_empty();
    match (lines.iter().position(|line| !blank(line)), lines.iter().rposition(|line| !blank(line)))
    {
        (Some(start), Some(end)) => lines[start..=end].to_vec(),
        _ => Vec::new(),
    }
}

/// One line of a command, safe to print: a tab becomes a space, and every other control
/// character, a carriage return too, a visible stand-in, because the shell does not
/// read a carriage return as a space.
pub(crate) fn command_line(line: &str) -> String {
    line.chars().map(|c| if c == '\t' { ' ' } else { visible(c) }).collect()
}

/// Each line of a command of several lines, numbered and after `indent`, so the start
/// of each line shows also where a terminal wraps a long one; `None` for a command of
/// one line.
pub(crate) fn numbered_lines(command: &str, indent: &str) -> Option<Vec<String>> {
    let lines = command_lines(command);
    if lines.len() < 2 {
        return None;
    }
    let digits = lines.len().to_string().len();
    let numbered = lines
        .iter()
        .enumerate()
        .map(|(at, line)| {
            let number = at + 1;
            format!("{indent}{number:>digits$}  {}", command_line(line)).trim_end().to_owned()
        })
        .collect();
    Some(numbered)
}

/// The heading of a question about `tool` running `command`: `shell: run "ls -la"`, or
/// for a command of several lines `shell: run 2 lines:` and then each line on its own,
/// numbered. The whole command shows: the user approves exactly what runs.
pub(crate) fn run_heading(tool: &str, command: &str) -> Vec<String> {
    let tool = one_line(tool);
    match numbered_lines(command, "  ") {
        Some(numbered) => {
            let mut lines = vec![format!("{tool}: run {} lines:", numbered.len())];
            lines.extend(numbered);
            lines
        }
        None => {
            let line = command_lines(command)
                .first()
                .map_or_else(String::new, |line| command_line(line.trim_end_matches('\r')));
            vec![format!("{tool}: run \"{line}\"")]
        }
    }
}

/// `text` cut to `columns` with `…` at the cut; whole when it fits.
fn cut(text: &str, columns: usize) -> String {
    if width(text) <= columns {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > columns {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('\u{2026}');
    out
}

/// How many columns `text` takes on a terminal.
fn width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// The end of a tool call worth a line: a failure or a non-zero exit. `None` for a
/// call that went well.
pub(crate) fn tool_result(tool: &str, is_error: bool, exit_code: Option<i32>) -> Option<String> {
    match exit_code {
        Some(code) if code != 0 => Some(format!("{tool} exited with {code}")),
        _ if is_error => Some(format!("{tool} failed")),
        _ => None,
    }
}

/// The line of a call that efr refused before it ran, such as `shell refused: efr's
/// config (floor)`, with the daemon's reason.
pub(crate) fn refused(tool: &str, reason: &str) -> String {
    format!("{} refused: {}", one_line(tool), one_line(reason))
}

/// A turn's settings on one line, such as `mode auto, model gpt-5.4, effort high`: every
/// value with `all`, else only the ones that the prompt set. `None` when that leaves
/// none. An effort that is not sent shows as `default`, the backend's.
pub(crate) fn turn_settings(settings: &EffectiveSettings, all: bool) -> Option<String> {
    let EffectiveSettings { mode, model, effort, overridden, .. } = settings;
    let mut parts = Vec::new();
    if all || overridden.mode {
        parts.push(format!("mode {mode}"));
    }
    if all || overridden.model {
        parts.push(format!("model {}", one_line(model)));
    }
    if all || overridden.effort {
        parts.push(format!("effort {}", one_line(effort.as_deref().unwrap_or("default"))));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The answer to an approval in a word.
pub(crate) fn decision(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Allow => "allowed",
        ApprovalDecision::Deny => "denied",
        _ => "answered",
    }
}

/// The surface that did something, as the user knows it.
pub(crate) fn origin(origin: Origin) -> &'static str {
    match origin {
        Origin::Shell => "the shell",
        Origin::Cli => "efr",
        Origin::Proxy => "a terminal proxy",
        Origin::Phone => "the phone",
        _ => "another client",
    }
}

/// A conversation's status in the words of the listing.
fn status_word(status: ConversationStatus) -> &'static str {
    match status {
        ConversationStatus::Idle => "idle",
        ConversationStatus::Running => "running",
        ConversationStatus::AwaitingApproval => "awaiting approval",
        _ => "unknown",
    }
}

/// How long ago `then` was at `now`, such as `5m ago`; `just now` under a second.
pub(crate) fn ago(then: Timestamp, now: Timestamp) -> String {
    let seconds = now.duration_since(then).as_secs();
    if seconds < 1 {
        return "just now".to_owned();
    }
    format!("{} ago", span(seconds.unsigned_abs()))
}

/// How long until `then` from `now`, such as `in 52m`, or how long ago it passed.
pub(crate) fn until(then: Timestamp, now: Timestamp) -> String {
    let seconds = then.duration_since(now).as_secs();
    if seconds < 0 { ago(then, now) } else { format!("in {}", span(seconds.unsigned_abs())) }
}

/// A duration in its two largest units: `45s`, `5m 3s`, `2h 5m`, `3d 4h`.
fn span(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    let secs = seconds % 60;
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, _) => format!("{minutes}m {secs}s"),
        (0, _, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

/// `efr status`: the daemon's identity and health, one fact per line.
pub(crate) fn status(status: &AdminStatusResult, socket: &Path, now: Timestamp) -> String {
    let mut out = String::new();
    let mut row = |key: &str, value: &str| {
        let _ = writeln!(out, "{key:<14} {value}");
    };
    row(
        "daemon",
        &format!(
            "efrd {}, protocol {}, pid {}",
            one_line(&status.version),
            status.protocol,
            status.pid
        ),
    );
    row("daemon id", &status.daemon_id.to_string());
    // Whole seconds: the nanoseconds of a start time say nothing to a person.
    row("started", &format!("{:.0} ({})", status.started_at, ago(status.started_at, now)));
    row("socket", &one_line(&socket.display().to_string()));
    row("screen", &one_line(&status.screen_backend));
    row("conversations", &status.conversations.to_string());
    row("shells", &status.shells.to_string());
    if status.providers.is_empty() {
        row("providers", "none configured");
    }
    for provider in &status.providers {
        let state = match (provider.logged_in, provider.expires_at) {
            (false, _) => "not logged in".to_owned(),
            (true, None) => "logged in".to_owned(),
            (true, Some(expires)) => {
                format!("logged in, token expires {expires:.0} ({})", until(expires, now))
            }
        };
        row("provider", &format!("{}: {state}", one_line(&provider.provider)));
    }
    if let Some(config) = &status.config {
        let file = match (&config.symlink_target, config.exists) {
            (Some(target), _) => format!("{} -> {}", config.path.display(), target.display()),
            (None, true) => config.path.display().to_string(),
            (None, false) => format!("{} (absent)", config.path.display()),
        };
        row("config", &one_line(&file));
        if let Some(error) = &config.reload_error {
            row("config error", &config_error(error));
        }
        if !config.restart_needed.is_empty() {
            row("restart needed", &one_line(&config.restart_needed.join(", ")));
        }
    }
    // NOTE: a daemon from before the sandbox reports none, so there is nothing to show.
    if let Some(sandbox) = &status.sandbox {
        row("sandbox", &sandbox::status_line(sandbox));
        if let Some(fix) = sandbox.fix.as_deref().filter(|_| !sandbox.available) {
            row("sandbox fix", &one_line(fix));
        }
    }
    out
}

/// A config file's error as one line: the message, then its key and place.
pub(crate) fn config_error(error: &ConfigFileError) -> String {
    let place = config_error_place(error);
    if place.is_empty() {
        one_line(&error.message)
    } else {
        one_line(&format!("{} ({place})", error.message))
    }
}

/// The key and the place of a config file's error, such as `shell.login, line 3,
/// column 1`, or nothing when it has neither.
pub(crate) fn config_error_place(error: &ConfigFileError) -> String {
    let mut parts = Vec::new();
    if let Some(key) = &error.key {
        parts.push(key.clone());
    }
    match (error.line, error.column) {
        (Some(line), Some(column)) => parts.push(format!("line {line}, column {column}")),
        (Some(line), None) => parts.push(format!("line {line}")),
        _ => {}
    }
    parts.join(", ")
}

/// The outcome of a reload, for `efr config reload` and the commands that change the
/// file.
pub(crate) fn reloaded(result: &AdminConfigReloadResult) -> String {
    let mut out = match (&result.error, result.applied) {
        (_, true) => "config.toml reloaded; new turns use it\n".to_owned(),
        (Some(error), false) => {
            format!("config.toml has an error; the old settings stay: {}\n", config_error(error))
        }
        (None, false) => "config.toml was not applied; the old settings stay\n".to_owned(),
    };
    if !result.restart_needed.is_empty() {
        let _ = writeln!(
            out,
            "restart efrd to apply: {} (systemctl --user restart efrd)",
            one_line(&result.restart_needed.join(", "))
        );
    }
    out
}

/// `efr history` without a conversation: one line per conversation, newest first.
pub(crate) fn conversations(list: &ConversationsListResult, now: Timestamp) -> String {
    if list.conversations.is_empty() {
        return "no conversations yet\n".to_owned();
    }
    let mut out = String::new();
    for summary in &list.conversations {
        let title = summary.title.as_deref().map_or_else(|| "(untitled)".to_owned(), one_line);
        // The age is at most 11 characters wide (`59m 59s ago`, `23h 59m ago`), so the
        // titles start in one column.
        let _ = writeln!(
            out,
            "{}  {:<17}  {:>11}  {title}",
            summary.id,
            status_word(summary.status),
            ago(summary.updated_at, now)
        );
    }
    if let Some(cursor) = &list.next_cursor {
        let _ = writeln!(out, "more: efr history --cursor {}", one_line(cursor.as_str()));
    }
    out
}

#[cfg(test)]
mod tests;
