//! Running one command line in a hidden shell: the request, the result, progress, and
//! the run delimited by OSC 133 marks.
//!
//! A marked run types the command as one bracketed paste and Enter, waits for `C`
//! (`OutputStart`), keeps the bytes after it, and ends at `D` (`CommandEnd`); the
//! output is `recording[C.end .. D.start]`. The sentinel run for shells without the
//! integration is in `sentinel.rs`.
//!
//! A sandboxed run (the auto mode) types a fixed wrapper line instead of the command,
//! and its end does not trust the marks of its output: see [`SandboxWatch`].

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use efr_protocol::{CallId, InputWait, ScreenSnapshot, Seq};
use efr_sandbox::SandboxResult;
use efr_screen::{PromptKind, SemanticPromptEvent, ShellMark, ShellMarkKind, row_text};

use crate::capture::{Capture, Kept};
use crate::{SandboxRun, ShellError};

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
    /// The tool call that runs the command, if any. Only an answer for this call
    /// reaches the command while it runs (see
    /// [`ShellSessions::answer`](crate::ShellSessions::answer)).
    pub call: Option<CallId>,
    /// Make the shell forget sudo's and doas's cached credentials when the command
    /// ends, before anything else runs there (`shell.sudo_cache = "per_call"`). It
    /// takes effect for a run delimited by marks: the integration's key runs
    /// `sudo -k` and `doas -L` without printing anything.
    pub forget_credentials: bool,
    /// The longest the run waits for its command in all, past [`timeout`](Self::timeout),
    /// while [`RunProgress::can_answer`] says that a person who can type answers
    /// follows it: for a command that the user approved because it may wait for input.
    /// `None` keeps the timeout.
    pub interactive_limit: Option<Duration>,
    /// Runs the command through the auto mode's launcher instead of in the shell: the
    /// command goes to the call's `line` file, and the shell gets only a fixed wrapper
    /// line with the call's id. `None` runs it in the shell, as every other mode does.
    pub sandbox: Option<SandboxRun>,
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
            call: None,
            forget_credentials: false,
            interactive_limit: None,
            sandbox: None,
        }
    }

    /// Runs the command through the auto mode's launcher with `run`, or in the shell
    /// with `None`.
    #[must_use]
    pub fn with_sandbox(mut self, run: Option<SandboxRun>) -> Self {
        self.sandbox = run;
        self
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

    /// Sets the tool call that runs the command.
    #[must_use]
    pub fn with_call(mut self, call: CallId) -> Self {
        self.call = Some(call);
        self
    }

    /// Sets whether the shell forgets the cached sudo and doas credentials when the
    /// command ends.
    #[must_use]
    pub fn with_forget_credentials(mut self, forget: bool) -> Self {
        self.forget_credentials = forget;
        self
    }

    /// Sets the longest the run waits for its command while a person who can answer
    /// follows it; `None` keeps the timeout.
    #[must_use]
    pub fn with_interactive_limit(mut self, limit: Option<Duration>) -> Self {
        self.interactive_limit = limit;
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
    /// a password or a `[Y/n]` prompt. The command keeps running; nothing can answer
    /// it through this run any more, and the next run waits for its prompt.
    Interactive,
    /// The timeout passed while a full-screen program (an editor, a pager, `top`) runs
    /// on the alternate screen. Nobody can reach it until something attaches to the
    /// shell, so it keeps running until it ends by itself; the next run waits for its
    /// prompt.
    FullScreen,
    /// The timeout passed while the command still runs and does not look like it is
    /// waiting for input.
    StillRunning,
    /// The command waited for hidden input, such as a password, while
    /// [`RunProgress::can_answer_hidden`] said that nobody could answer it, so the run
    /// sent `SIGINT` to the terminal's foreground process group and returned at once.
    /// When the command had ended by then and the shell held the terminal again, no
    /// signal was sent. The command may still be ending; the next run waits for its
    /// prompt. A sandboxed call that waits for hidden input always ends this way: efr
    /// never types a secret into a sandboxed command.
    Unanswered,
    /// A sandboxed run whose sandbox did not start or did not report: the wrapper check
    /// failed or the integration is missing (nothing ran), the sandbox's setup failed
    /// ([`SandboxResult::setup_error`](efr_sandbox::SandboxResult::setup_error)), or the
    /// launcher failed after it started the command, which may have run
    /// ([`SandboxResult::launch_error`](efr_sandbox::SandboxResult::launch_error), also
    /// when the launcher died before its result). The command never ran outside the
    /// sandbox.
    SandboxFailed,
}

/// The result of [`ShellSessions::run_command`](crate::ShellSessions::run_command).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommandResult {
    /// How the run ended.
    pub completion: Completion,
    /// The exit status, for a finished command.
    pub exit_code: Option<i32>,
    /// The output as plain text, so far for a command that still runs. Output that
    /// moves the cursor (a progress display redrawn in place, a full-screen program)
    /// is what a screen of the shell's width shows at the end, without trailing
    /// blanks; other output is its bytes without escape sequences. When it was over
    /// the request's output limit, a marker line stands for the middle.
    pub output: String,
    /// True when bytes were left out of [`output`](Self::output).
    pub truncated: bool,
    /// The size of the whole output so far, in bytes.
    pub output_bytes: u64,
    /// Where the output lies in the PTY recording, when its start is known; the full
    /// output can be read back from there.
    pub output_range: Option<Range<Seq>>,
    /// The shell's working directory at the end of the run. For a sandboxed run it is
    /// the directory from the launcher's `result.json`, which may lie in the sandbox's
    /// private tmp (see [`ShellState::sandbox_cwd`](crate::ShellState::sandbox_cwd)).
    pub cwd_after: PathBuf,
    /// The last lines of the screen, for a command that still runs.
    pub screen_tail: Option<String>,
    /// What delimited the output.
    pub delimiter: Delimiter,
    /// The launcher's `result.json`, for a sandboxed run that ended with one.
    pub sandbox: Option<SandboxResult>,
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
            sandbox: None,
        }
    }

    /// Sets the launcher's result of a sandboxed run.
    #[must_use]
    pub fn with_sandbox(mut self, result: SandboxResult) -> Self {
        self.sandbox = Some(result);
        self
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

    /// True when the command waits for input at the terminal: a prompt or a
    /// full-screen program.
    pub fn interactive(&self) -> bool {
        matches!(self.completion, Completion::Interactive | Completion::FullScreen)
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

/// Hears a command's output as it grows, and whether it waits for input. Updates are
/// coalesced: a slow listener gets the latest state, never a backlog.
pub trait RunProgress: Send {
    /// Takes the latest state of the output.
    fn update(&mut self, update: &OutputUpdate);

    /// The command started or stopped waiting for input. Each change comes once, and a
    /// run that ended or was left while it waited reports [`InputWait::None`] last.
    /// `looks_secret` is true for a visible wait whose prompt reads like a password
    /// prompt while the terminal is not in line mode, as behind a relay; a change of it
    /// alone is a change too. Ignored by default.
    fn input_changed(&mut self, _wait: InputWait, _looks_secret: bool) {}

    /// Whether a person can answer hidden input for this run now. Asked when the
    /// command starts to wait for hidden input and at every look while it waits;
    /// `false` stops the command ([`Completion::Unanswered`]). True by default, which
    /// lets the command wait until it ends or the timeout passes.
    fn can_answer_hidden(&mut self) -> bool {
        true
    }

    /// Whether a person who can type answers follows the run now. Asked at the timeout,
    /// and then once per quiet period, of a run with an
    /// [`interactive_limit`](RunRequest::interactive_limit): `true` keeps it waiting for
    /// its command up to that limit. False by default, which keeps the timeout.
    fn can_answer(&mut self) -> bool {
        false
    }
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
    /// True once the command runs: its output has started.
    pub(crate) started: bool,
}

impl Progress {
    pub(crate) fn of(capture: &Capture, started: bool) -> Self {
        Progress { bytes: capture.total(), tail: capture.tail(PREVIEW_BYTES), started }
    }

    pub(crate) fn update(&self) -> OutputUpdate {
        OutputUpdate { bytes: self.bytes, tail: crate::capture::clean(&self.tail) }
    }

    /// The bytes of the tail that a screen reads. A tail that starts inside the output
    /// starts after its first line feed, when it has one, so the screen never starts in
    /// the middle of an escape sequence and prints its rest as text.
    pub(crate) fn window(&self) -> Bytes {
        if self.bytes <= self.tail.len() as u64 {
            return self.tail.clone();
        }
        match self.tail.iter().position(|&byte| byte == b'\n') {
            Some(at) if at + 1 < self.tail.len() => self.tail.slice(at + 1..),
            _ => self.tail.clone(),
        }
    }
}

/// The key the integration binds to a widget that empties the line editor, so text
/// typed at the attached screen and never sent cannot join the command.
pub(crate) const CLEAR_LINE: &[u8] = b"\x1b[efr-clear~";

/// The key the integration binds to a widget that makes the shell forget the cached
/// sudo and doas credentials (`sudo -k`, `doas -L`) without printing anything. It is
/// typed at the prompt after the run's end, before the next line, so nothing runs in
/// between.
pub(crate) const FORGET_CREDENTIALS: &[u8] = b"\x1b[efr-forget~";

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

/// The fixed line of a sandboxed call, up to the call id. It runs the wrapper only
/// when the three functions of the wrapper are the ones the integration defined and no
/// function shadows `builtin` or `command`. The backslash stops alias expansion of the
/// wrapper's name. zsh expands an alias named `[[` even though `[[` is a reserved word,
/// so the clear key that comes right before this line removes that alias with the
/// global and suffix aliases (`_efr_hs_plain_words`). Without the integration zsh
/// answers `command not found`. Either way nothing runs and no end mark comes.
pub(crate) const WRAPPER_CHECK: &str = r#"[[ "${functions[_efr_hs_sbx]-}${functions[_efr_hs_sbx_apply]-}${functions[_efr_hs_sbx_snapshot]-}" == "$_efr_hs_sbx_src" && -z ${functions[builtin]-}${functions[command]-} ]] && \_efr_hs_sbx "#;

/// The bytes that type the fixed line of a sandboxed call: the clear key, one
/// bracketed paste with [`WRAPPER_CHECK`] and the call id, then Enter. The model's line
/// is never typed; it goes to the call's `line` file.
pub(crate) fn wrapped_line(call: CallId) -> Result<Bytes, ShellError> {
    marked_line(&format!("{WRAPPER_CHECK}{call}"))
}

/// How much of what comes before `C` a marked run keeps.
const BEFORE_LIMIT: usize = 8 * 1024;

/// The launch error of a sandboxed run whose launcher wrote `started` but no result.
const LAUNCHER_LOST: &str = "the launcher ended after the command started, without its result";

/// What a sandboxed run watches for its end (efr's auto spec, section 3.15).
///
/// The sandboxed command shares the terminal, so it can print any mark, a fake `D`
/// included. The run therefore ends only on facts that sandboxed code cannot make:
/// the end mark with the call's nonce, which only the trusted wrapper prints after the
/// launcher returned; a `D` after it; the shell's own process group in the terminal's
/// foreground; and the launcher's `started` and `result.json` files. The marks between
/// `C` and the end mark are not applied to the shell's state.
#[derive(Debug, Clone)]
pub(crate) struct SandboxWatch {
    nonce: [u8; 16],
    /// The call's dir, where the launcher writes its files.
    dir: PathBuf,
    /// True for a contained call, false for the exit child of an approved exit.
    contained: bool,
    /// The start of the call's end mark, once it came after `C`.
    end_mark: Option<Seq>,
    /// How many captured bytes the output keeps when the run ends at the pending check.
    keep: Option<u64>,
}

impl SandboxWatch {
    pub(crate) fn new(run: &SandboxRun) -> Self {
        SandboxWatch {
            nonce: run.nonce,
            dir: run.dir.clone(),
            contained: run.contained(),
            end_mark: None,
            keep: None,
        }
    }
}

/// A `D` of a sandboxed run, which ends the run only when the facts agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// A `D` before the end mark. The run ends at once as
    /// [`Completion::SandboxFailed`] when the shell holds the terminal and the launcher
    /// never started: the wrapper check failed or the integration is missing, so no
    /// sandboxed code ran. Otherwise the `D` may be sandboxed code's, and the run goes
    /// on.
    Early {
        /// The status that the `D` carries.
        exit_code: Option<i32>,
        /// Where the `D` starts.
        at: Seq,
    },
    /// A `D` after the end mark: the run ends when the shell holds the terminal, as a
    /// finished run with `result.json`, or as [`Completion::SandboxFailed`] without it.
    Final {
        /// The status that the `D` carries: the wrapper's, which is the launcher's.
        exit_code: Option<i32>,
        /// Where the end mark starts, which ends the output.
        at: Seq,
    },
}

/// What the session's actor read for a [`Check`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The shell's own process group holds the terminal.
    pub(crate) shell_holds_terminal: bool,
    /// `$CALL/started` exists.
    pub(crate) started: bool,
    /// `$CALL/result.json`, when it exists and reads.
    pub(crate) result: Option<SandboxResult>,
}

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
    /// A sandboxed run saw a `D`: the actor reads the facts, then the run ends or goes
    /// on ([`MarkRun::checked`]).
    Check(Check),
}

/// The output of a run that ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunOutput {
    pub(crate) completion: Completion,
    pub(crate) exit_code: Option<i32>,
    /// The raw bytes. The caller turns them into text off the session's actor, because
    /// output that moves the cursor is replayed on a screen first.
    pub(crate) kept: Kept,
    pub(crate) range: Option<Range<Seq>>,
    /// The directory from the sentinel's end line, or from a sandboxed run's
    /// `result.json`; marks report it through the state.
    pub(crate) cwd: Option<PathBuf>,
    /// The launcher's result of a sandboxed run.
    pub(crate) sandbox: Option<Box<SandboxResult>>,
}

impl RunOutput {
    /// A sandboxed run that never reached its shell: the shell has no marks, so the
    /// wrapper line cannot be typed and checked.
    pub(crate) fn sandbox_refused() -> Self {
        RunOutput {
            completion: Completion::SandboxFailed,
            exit_code: None,
            kept: Capture::new(0).finish(),
            range: None,
            cwd: None,
            sandbox: None,
        }
    }
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
    /// Type [`FORGET_CREDENTIALS`] when the run ends.
    forget_credentials: bool,
    /// Set for a sandboxed run, whose end does not trust its output's marks.
    sandbox: Option<SandboxWatch>,
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
            forget_credentials: false,
            sandbox: None,
        }
    }

    /// The same run, for a sandboxed call that `watch` describes.
    pub(crate) fn sandboxed(mut self, watch: SandboxWatch) -> Self {
        self.sandbox = Some(watch);
        self
    }

    /// True while the marks of the stream may come from sandboxed code: between the
    /// `C` of a sandboxed run and its end mark. The shell's state ignores them then.
    pub(crate) fn shields(&self) -> bool {
        self.output_start.is_some()
            && self.sandbox.as_ref().is_some_and(|watch| watch.end_mark.is_none())
    }

    /// The call's dir of a sandboxed run.
    pub(crate) fn sandbox_dir(&self) -> Option<&Path> {
        self.sandbox.as_ref().map(|watch| watch.dir.as_path())
    }

    /// True for a sandboxed run whose command runs in the sandbox, not in the exit
    /// child of an approved exit.
    pub(crate) fn contained(&self) -> bool {
        self.sandbox.as_ref().is_some_and(|watch| watch.contained)
    }

    /// The same run, typing [`FORGET_CREDENTIALS`] when it ends when `forget` is set.
    pub(crate) fn forgetting(mut self, forget: bool) -> Self {
        self.forget_credentials = forget;
        self
    }

    /// True when the shell must forget the cached credentials once the run ends.
    pub(crate) fn forgets_credentials(&self) -> bool {
        self.forget_credentials
    }

    /// Takes stream bytes that lie between marks. Returns true when they were output.
    pub(crate) fn on_bytes(&mut self, at: Seq, bytes: &[u8]) -> bool {
        if self.output_start.is_none() {
            self.before.push(bytes);
            return false;
        }
        // After a sandboxed call's end mark only the wrapper and the prompt print.
        if bytes.is_empty() || self.sandbox.as_ref().is_some_and(|watch| watch.end_mark.is_some()) {
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
        // Before `C` only the trusted shell prints, so a sandboxed run reads those marks
        // as any run does.
        if self.output_start.is_some()
            && let Some(watch) = &self.sandbox
        {
            return self.on_sandboxed_mark(mark, watch.end_mark, watch.nonce);
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

    /// A mark after the `C` of a sandboxed run. Only the call's own end mark and a `D`
    /// count; every other mark may be sandboxed code's.
    fn on_sandboxed_mark(
        &mut self,
        mark: &ShellMark,
        end_mark: Option<Seq>,
        nonce: [u8; 16],
    ) -> MarkStep {
        match &mark.kind {
            ShellMarkKind::SandboxEnd { nonce: seen } if end_mark.is_none() && *seen == nonce => {
                let keep = self.keep_before(mark.start);
                if let Some(watch) = &mut self.sandbox {
                    watch.end_mark = Some(mark.start);
                    watch.keep = Some(keep);
                }
                MarkStep::Continue
            }
            ShellMarkKind::SemanticPrompt(SemanticPromptEvent::CommandEnd {
                exit_code, ..
            }) => {
                let exit_code = *exit_code;
                match end_mark {
                    Some(at) => MarkStep::Check(Check::Final { exit_code, at }),
                    None => {
                        let keep = self.keep_before(mark.start);
                        if let Some(watch) = &mut self.sandbox {
                            watch.keep = Some(keep);
                        }
                        MarkStep::Check(Check::Early { exit_code, at: mark.start })
                    }
                }
            }
            _ => MarkStep::Continue,
        }
    }

    /// How many captured bytes lie before the mark that starts at `at`. Its first bytes
    /// may sit in the capture already, read before the scanner could tell it was a mark.
    fn keep_before(&self, at: Seq) -> u64 {
        let overshoot = self.captured_end.get().saturating_sub(at.get());
        self.capture.total().saturating_sub(overshoot)
    }

    /// Ends a sandboxed run at `check` when `facts` agree (efr's auto spec, section
    /// 3.15); `None` means the run goes on until its timeout, as any other run whose
    /// end is not known.
    pub(crate) fn checked(&mut self, check: Check, facts: &Facts) -> Option<RunOutput> {
        let start = self.output_start?;
        if !facts.shell_holds_terminal {
            return None;
        }
        let (exit_code, at, completion, sandbox) = match check {
            Check::Early { exit_code, at } => {
                if facts.started {
                    return None;
                }
                (exit_code, at, Completion::SandboxFailed, None)
            }
            Check::Final { exit_code, at } => match &facts.result {
                Some(result) if result.setup_error.is_none() && result.launch_error.is_none() => {
                    (exit_code, at, Completion::Finished, Some(Box::new(result.clone())))
                }
                Some(result) => {
                    (exit_code, at, Completion::SandboxFailed, Some(Box::new(result.clone())))
                }
                // NOTE: a launcher that died after `started` (OOM, SIGKILL) may have run
                // the command, and the model must not read that it did not.
                None if facts.started => {
                    let lost = SandboxResult {
                        started: true,
                        launch_error: Some(LAUNCHER_LOST.to_owned()),
                        ..SandboxResult::default()
                    };
                    (exit_code, at, Completion::SandboxFailed, Some(Box::new(lost)))
                }
                None => (exit_code, at, Completion::SandboxFailed, None),
            },
        };
        let keep = self.sandbox.as_ref().and_then(|watch| watch.keep).unwrap_or(u64::MAX);
        let extra = self.capture.total().saturating_sub(keep);
        self.capture.trim_end(usize::try_from(extra).unwrap_or(usize::MAX));
        let cwd = sandbox.as_ref().and_then(|result| result.cwd.clone());
        Some(RunOutput {
            completion,
            exit_code,
            kept: self.capture.finish(),
            range: Some(start..at.max(start)),
            cwd,
            sandbox,
        })
    }

    pub(crate) fn capture(&self) -> &Capture {
        &self.capture
    }

    /// True once `C` arrived: the command runs until `D` ends the run.
    pub(crate) fn running(&self) -> bool {
        self.output_start.is_some()
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
                sandbox: None,
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
            sandbox: None,
        }
    }
}

/// How a run whose command still runs at its timeout ended, from the screen and
/// whether the output was `quiet` for `quiet_period`. A full-screen program on the
/// alternate screen counts whether or not it redraws: nobody can reach it either way.
/// A quiet command waits for input when the cursor sits after some text on its row,
/// as after `Password:` or `[Y/n] `; a command that is merely slow has usually ended
/// its last line, which leaves the cursor at the start of the next.
pub(crate) fn timed_out(snapshot: &ScreenSnapshot, quiet: bool) -> Completion {
    if snapshot.alternate_screen {
        Completion::FullScreen
    } else if quiet && cursor_after_text(snapshot) {
        Completion::Interactive
    } else {
        Completion::StillRunning
    }
}

/// True when the cursor sits after some text on its row.
pub(crate) fn cursor_after_text(snapshot: &ScreenSnapshot) -> bool {
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
