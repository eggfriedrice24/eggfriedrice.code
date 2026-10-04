//! Running one command line in a hidden shell: the request, the result, progress, and
//! the run delimited by OSC 133 marks.
//!
//! A marked run types the command as one bracketed paste and Enter, waits for `C`
//! (`OutputStart`), keeps the bytes after it, and ends at `D` (`CommandEnd`); the
//! output is `recording[C.end .. D.start]`. The sentinel run for shells without the
//! integration is in `sentinel.rs`.

use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use efr_protocol::{ScreenSnapshot, Seq};
use efr_screen::{PromptKind, SemanticPromptEvent, ShellMark, ShellMarkKind, row_text};

use crate::ShellError;
use crate::capture::{Capture, Kept};

/// One command line for a conversation's hidden shell.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunRequest {
    /// The command line as the model wrote it. It may span several lines.
    pub command: String,
    /// The directory a new shell starts in, when the conversation has none yet. A
    /// running shell stays wherever it is.
    pub start_dir: PathBuf,
    /// How long to wait for the command to end before returning what is known.
    pub timeout: Duration,
    /// How the command is delimited.
    pub mode: RunMode,
    /// The most output bytes kept in memory: half from the start, half from the end.
    pub output_limit: usize,
}

impl RunRequest {
    /// The timeout unless the request says otherwise.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
    /// The output limit unless the request says otherwise.
    pub const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;

    /// A request with the default timeout, mode and output limit.
    pub fn new(command: impl Into<String>, start_dir: impl Into<PathBuf>) -> Self {
        RunRequest {
            command: command.into(),
            start_dir: start_dir.into(),
            timeout: Self::DEFAULT_TIMEOUT,
            mode: RunMode::Auto,
            output_limit: Self::DEFAULT_OUTPUT_LIMIT,
        }
    }

    /// Sets the timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the mode.
    #[must_use]
    pub fn with_mode(mut self, mode: RunMode) -> Self {
        self.mode = mode;
        self
    }

    /// Sets the output limit.
    #[must_use]
    pub fn with_output_limit(mut self, output_limit: usize) -> Self {
        self.output_limit = output_limit;
        self
    }
}

/// How a run finds where its command's output starts and ends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RunMode {
    /// OSC 133 marks when the shell has the integration, typed once it sits at its
    /// prompt; sentinels when it has no integration.
    #[default]
    Auto,
    /// Always sentinels, typed into whatever runs in the foreground. This is for a
    /// shell started inside the hidden one (`sudo -i`, `bash`, `ssh`), which has no
    /// efr integration and keeps the hidden zsh's command running.
    Sentinel,
}

/// What delimited a run's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Delimiter {
    /// OSC 133 `C` and `D` from the integration.
    Marks,
    /// A random-token `printf` before and after the command.
    Sentinel,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Completion {
    /// The command ended; [`CommandResult::exit_code`] says how.
    Finished,
    /// The line ran no command: it was empty, did not parse, or was unfinished (an
    /// unclosed quote) and was cancelled.
    NotStarted,
    /// The timeout passed while the command waits for input at the terminal, such as
    /// a password or a `[Y/n]` prompt. The user can answer on the attached screen.
    Interactive,
    /// The timeout passed while the command still runs and does not look like it is
    /// waiting for input.
    StillRunning,
}

/// The result of [`ShellSessions::run_command`](crate::ShellSessions::run_command).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommandResult {
    /// How the run ended.
    pub completion: Completion,
    /// The exit status, for a finished command.
    pub exit_code: Option<i32>,
    /// The output as plain text, so far for a command that still runs. When it was
    /// over the request's output limit, a marker line stands for the middle.
    pub output: String,
    /// True when bytes were left out of [`output`](Self::output).
    pub truncated: bool,
    /// The size of the whole output so far, in bytes.
    pub output_bytes: u64,
    /// Where the output lies in the PTY recording, when its start is known; the full
    /// output can be read back from there.
    pub output_range: Option<Range<Seq>>,
    /// The shell's working directory at the end of the run.
    pub cwd_after: PathBuf,
    /// The last lines of the screen, for a command that still runs.
    pub screen_tail: Option<String>,
    /// What delimited the output.
    pub delimiter: Delimiter,
}

impl CommandResult {
    /// A finished, marked run with nothing truncated and no recording range. The
    /// setters below change the rest. [`ShellSessions`](crate::ShellSessions) builds
    /// its results itself; this is for other [`CommandRunner`](crate::CommandRunner)s,
    /// such as the fakes in other crates' tests.
    pub fn finished(
        exit_code: Option<i32>,
        output: impl Into<String>,
        cwd_after: impl Into<PathBuf>,
    ) -> Self {
        let output = output.into();
        CommandResult {
            completion: Completion::Finished,
            exit_code,
            output_bytes: output.len() as u64,
            output,
            truncated: false,
            output_range: None,
            cwd_after: cwd_after.into(),
            screen_tail: None,
            delimiter: Delimiter::Marks,
        }
    }

    /// Sets how the run ended.
    #[must_use]
    pub fn with_completion(mut self, completion: Completion) -> Self {
        self.completion = completion;
        self
    }

    /// Sets the screen's last lines.
    #[must_use]
    pub fn with_screen_tail(mut self, screen_tail: impl Into<String>) -> Self {
        self.screen_tail = Some(screen_tail.into());
        self
    }

    /// Marks the output as truncated, with `output_bytes` the size of the whole.
    #[must_use]
    pub fn with_truncation(mut self, output_bytes: u64) -> Self {
        self.truncated = true;
        self.output_bytes = output_bytes;
        self
    }

    /// Sets what delimited the output.
    #[must_use]
    pub fn with_delimiter(mut self, delimiter: Delimiter) -> Self {
        self.delimiter = delimiter;
        self
    }

    /// True when the command waits for input at the terminal.
    pub fn interactive(&self) -> bool {
        self.completion == Completion::Interactive
    }
}

/// A command's output so far, for a live preview.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutputUpdate {
    /// The size of the output so far, in bytes.
    pub bytes: u64,
    /// The end of the output so far as plain text, at most a few KiB.
    pub tail: String,
}

impl OutputUpdate {
    /// An update with `bytes` of output so far, ending in `tail`.
    pub fn new(bytes: u64, tail: impl Into<String>) -> Self {
        OutputUpdate { bytes, tail: tail.into() }
    }
}

/// Hears a command's output as it grows. Updates are coalesced: a slow listener gets
/// the latest state, never a backlog.
pub trait RunProgress: Send {
    /// Takes the latest state of the output.
    fn update(&mut self, update: &OutputUpdate);
}

impl<F: FnMut(&OutputUpdate) + Send> RunProgress for F {
    fn update(&mut self, update: &OutputUpdate) {
        self(update);
    }
}

/// A [`RunProgress`] that ignores every update.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl RunProgress for NoProgress {
    fn update(&mut self, _update: &OutputUpdate) {}
}

/// How many raw bytes of the output's end a progress update carries.
pub(crate) const PREVIEW_BYTES: usize = 4096;

/// The rows a screen tail shows at most.
pub(crate) const SCREEN_TAIL_ROWS: usize = 20;

/// The raw state behind an [`OutputUpdate`], as the session's actor publishes it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Progress {
    pub(crate) bytes: u64,
    pub(crate) tail: Bytes,
}

impl Progress {
    pub(crate) fn of(capture: &Capture) -> Self {
        Progress { bytes: capture.total(), tail: capture.tail(PREVIEW_BYTES) }
    }

    pub(crate) fn update(&self) -> OutputUpdate {
        OutputUpdate { bytes: self.bytes, tail: crate::capture::clean(&self.tail) }
    }
}

/// The key the integration binds to a widget that empties the line editor, so text
/// typed at the attached screen and never sent cannot join the command.
pub(crate) const CLEAR_LINE: &[u8] = b"\x1b[efr-clear~";

/// The bytes that type `command` at a zsh prompt: the clear key, one bracketed paste,
/// so newlines and tabs are inserted rather than acted on, then Enter.
pub(crate) fn marked_line(command: &str) -> Result<Bytes, ShellError> {
    const PASTE_START: &[u8] = b"\x1b[200~";
    const PASTE_END: &[u8] = b"\x1b[201~";
    if command.contains('\0') {
        return Err(ShellError::InvalidCommand { reason: "it contains a NUL byte" });
    }
    if command.contains("\x1b[201~") {
        return Err(ShellError::InvalidCommand {
            reason: "it contains the end of a bracketed paste",
        });
    }
    if command.trim().is_empty() {
        return Err(ShellError::InvalidCommand { reason: "it is empty" });
    }
    let mut line = Vec::with_capacity(
        CLEAR_LINE.len() + PASTE_START.len() + command.len() + PASTE_END.len() + 1,
    );
    line.extend_from_slice(CLEAR_LINE);
    line.extend_from_slice(PASTE_START);
    line.extend_from_slice(command.as_bytes());
    line.extend_from_slice(PASTE_END);
    line.push(b'\r');
    Ok(Bytes::from(line))
}

/// How much of what comes before `C` a marked run keeps.
const BEFORE_LIMIT: usize = 8 * 1024;

/// What a marked run does after a mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkStep {
    /// Keep going.
    Continue,
    /// The line was unfinished and sits at a continuation prompt: type the cancel key
    /// (bound to send-break by the integration); the shell then ends the line with a
    /// bare `D`.
    Cancel,
    /// The line ended.
    Ended(RunOutput),
}

/// The output of a run that ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunOutput {
    pub(crate) completion: Completion,
    pub(crate) exit_code: Option<i32>,
    /// The raw bytes; the caller turns them into text, off the session's actor.
    pub(crate) kept: Kept,
    pub(crate) range: Option<Range<Seq>>,
    /// The directory from the sentinel's end line; marks report it through the state.
    pub(crate) cwd: Option<PathBuf>,
}

/// A run delimited by the integration's marks.
#[derive(Debug)]
pub(crate) struct MarkRun {
    /// The stream offset when the line was typed: marks before it belong to earlier
    /// lines.
    typed_at: Seq,
    /// The end of `C`, once it arrived.
    output_start: Option<Seq>,
    /// The offset after the last byte captured.
    captured_end: Seq,
    capture: Capture,
    /// What the shell printed after the line was typed and before `C`: the echo of the
    /// line and, for a line that never runs, the shell's complaint about it.
    before: Capture,
    /// A continuation prompt started before `C`: the line was unfinished.
    unfinished: bool,
    cancelled: bool,
}

impl MarkRun {
    pub(crate) fn new(typed_at: Seq, output_limit: usize) -> Self {
        MarkRun {
            typed_at,
            output_start: None,
            captured_end: typed_at,
            capture: Capture::new(output_limit),
            before: Capture::new(BEFORE_LIMIT),
            unfinished: false,
            cancelled: false,
        }
    }

    /// Takes stream bytes that lie between marks. Returns true when they were output.
    pub(crate) fn on_bytes(&mut self, at: Seq, bytes: &[u8]) -> bool {
        if self.output_start.is_none() {
            self.before.push(bytes);
            return false;
        }
        if bytes.is_empty() {
            return false;
        }
        self.capture.push(bytes);
        self.captured_end = Seq::new(at.get().saturating_add(bytes.len() as u64));
        true
    }

    pub(crate) fn on_mark(&mut self, mark: &ShellMark) -> MarkStep {
        if mark.start < self.typed_at {
            return MarkStep::Continue;
        }
        let ShellMarkKind::SemanticPrompt(event) = &mark.kind else {
            return MarkStep::Continue;
        };
        match event {
            SemanticPromptEvent::OutputStart { .. } if self.output_start.is_none() => {
                self.output_start = Some(mark.end);
                self.captured_end = mark.end;
                MarkStep::Continue
            }
            SemanticPromptEvent::CommandEnd { exit_code, .. } => {
                MarkStep::Ended(self.end(mark.start, *exit_code))
            }
            SemanticPromptEvent::PromptStart { kind: PromptKind::Secondary, .. }
                if self.output_start.is_none() =>
            {
                self.unfinished = true;
                MarkStep::Continue
            }
            // The cancel key goes out at `B`, once the line editor is about to read:
            // the integration prints `P` and `B` from zle-line-init, and input that
            // arrives earlier than its end can be lost.
            SemanticPromptEvent::InputStart if self.unfinished && !self.cancelled => {
                self.cancelled = true;
                MarkStep::Cancel
            }
            _ => MarkStep::Continue,
        }
    }

    pub(crate) fn capture(&self) -> &Capture {
        &self.capture
    }

    /// The output so far, for a run that is left running.
    pub(crate) fn partial(&self) -> (Kept, Option<Range<Seq>>) {
        let range = self.output_start.map(|start| start..self.captured_end);
        (self.capture.finish(), range)
    }

    fn end(&mut self, end: Seq, exit_code: Option<i32>) -> RunOutput {
        let Some(start) = self.output_start else {
            return RunOutput {
                completion: Completion::NotStarted,
                exit_code: None,
                kept: self.before.finish(),
                range: None,
                cwd: None,
            };
        };
        // The start of `D` may already sit in the capture, read before the scanner
        // could tell it was a mark.
        let overshoot = self.captured_end.get().saturating_sub(end.get());
        self.capture.trim_end(usize::try_from(overshoot).unwrap_or(usize::MAX));
        RunOutput {
            completion: Completion::Finished,
            exit_code,
            kept: self.capture.finish(),
            range: Some(start..end.max(start)),
            cwd: None,
        }
    }
}

/// True when a screen looks like it waits for input: a full-screen program on the
/// alternate screen, or the cursor after some text on its row, as after `Password:` or
/// `[Y/n] `. A command that is merely slow has usually ended its last line, which
/// leaves the cursor at the start of the next.
pub(crate) fn waits_for_input(snapshot: &ScreenSnapshot) -> bool {
    if snapshot.alternate_screen {
        return true;
    }
    let cursor = snapshot.cursor;
    if cursor.col == 0 {
        return false;
    }
    snapshot.rows.get(usize::from(cursor.row)).is_some_and(|row| !row_text(row).trim().is_empty())
}

/// The last rows of a screen up to the cursor's row, as text without trailing blank
/// rows.
pub(crate) fn screen_tail(snapshot: &ScreenSnapshot) -> String {
    let last = usize::from(snapshot.cursor.row).min(snapshot.rows.len().saturating_sub(1));
    let mut rows: Vec<String> = snapshot.rows.iter().take(last + 1).map(row_text).collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    let skip = rows.len().saturating_sub(SCREEN_TAIL_ROWS);
    rows[skip..].join("\n")
}

#[cfg(test)]
mod tests;
