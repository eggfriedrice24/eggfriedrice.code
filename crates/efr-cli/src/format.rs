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
use jiff::Timestamp;
use serde_json::Value;

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
    let named = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, ' ' | '.' | '_' | '/' | ':' | '@' | '%' | '+' | ',' | '-')
    };
    match summary.rsplit_once(ASKS_FOR) {
        Some((first, parts)) if !parts.is_empty() && parts.chars().all(named) => {
            (one_line(first), Some(format!("asks for: {parts}")))
        }
        _ => (one_line(summary), None),
    }
}

/// `text` safe to print as lines: newlines stay, tabs become a space, other control
/// characters visible stand-ins.
pub(crate) fn lines(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| c.is_control() && c != '\n') {
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

/// A control character's visible stand-in, the same mapping `efr-render` uses.
fn visible(c: char) -> char {
    match c {
        '\u{0}'..='\u{1f}' => char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'),
        '\u{7f}' => '\u{2421}',
        c if c.is_control() => '\u{fffd}',
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
pub(crate) fn tool_call(tool: &str, input: &Value) -> String {
    let detail = match input {
        Value::Object(members) => DETAIL_KEYS
            .iter()
            .find_map(|key| members.get(*key).and_then(Value::as_str))
            .map_or_else(|| input.to_string(), str::to_owned),
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    if detail.is_empty() { tool.to_owned() } else { format!("{tool}: {detail}") }
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
