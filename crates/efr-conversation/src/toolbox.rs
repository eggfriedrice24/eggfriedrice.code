//! The tools as a conversation sees them.
//!
//! The design names `efr_tools::ToolRegistry` as the conversation's tool layer, and the
//! crate allowlist permits `efr-conversation -> efr-tools`. But `efr-tools` depends on
//! `efr-shell`, and `xtask/src/deps.rs` forbids `efr-conversation -> efr-shell` through
//! any chain of dependencies, so this crate cannot name the registry. [`Toolbox`] is
//! the registry's shape in this crate's terms; the daemon implements it over its
//! `ToolRegistry`, which is a field-by-field copy (`ToolSpec` to [`ToolDefinition`],
//! `ToolRequirements` to [`Requirements`], `ToolResult` to [`ToolOutcome`]). The check
//! point stays in `turn.rs`: a toolbox declares, the engine decides, the turn enforces.

use std::fmt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use efr_permissions::{ExitNeed, Requirements};
use efr_protocol::{
    CallId, ConversationId, FileChanges, InputWait, Launch, Origin, ReportedFile, SandboxSummary,
    Scope, SurfaceChange, TurnId,
};
use efr_provider::ToolDefinition;
use serde_json::Value;

/// The tools a conversation offers the model.
///
/// The turn asks [`requirements`](Toolbox::requirements) first and hands the answer to
/// the permission engine; only an allowed or approved call reaches
/// [`invoke`](Toolbox::invoke). A toolbox never decides a permission itself.
#[async_trait]
pub trait Toolbox: Send + Sync + fmt::Debug {
    /// The tools for the provider request, in a stable order, so that the request stays
    /// the same from one model call to the next.
    fn definitions(&self) -> Vec<ToolDefinition>;

    /// What `call` needs: every path with its access, the command line it runs, network
    /// and terminal input. Nothing runs and nothing is written; a toolbox may read the
    /// file system to find what a path reaches through a symbolic link, off the async
    /// workers. `Err` is the text the model reads when the call cannot be judged, such
    /// as an unknown tool or an input that does not match the tool's schema.
    async fn requirements(&self, call: &ToolCall) -> Result<Requirements, String>;

    /// A diff of the change that `call` would make, for an approval request. Only a
    /// tool that writes a file has one; the default has none.
    async fn preview(&self, _call: &ToolCall) -> Option<String> {
        None
    }

    /// True when a call of the tool `name` with `input` takes an input that the user
    /// chooses to type while it reports no wait (`input.respond` with `manual`); the
    /// turn records it in the call's `tool_call_started`. Pure. The default takes none.
    fn takes_manual_input(&self, _name: &str, _input: &Value) -> bool {
        false
    }

    /// Runs `call`, which passed the permission check. Output that grows while it runs
    /// goes to `out`. A failure is an error outcome, which the model reads.
    async fn invoke(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome;

    /// Where the conversation's hidden shell is now, for the live-state preamble;
    /// `None` when no shell runs or the toolbox cannot tell. The turn then reads it
    /// from the event log, which a long conversation may have paged past. The default
    /// cannot tell.
    async fn shell_cwd(&self, _conversation_id: ConversationId) -> Option<PathBuf> {
        None
    }

    /// The jobs that run in the background of the conversation's hidden shell, one
    /// line each (its command line), for the fresh context block after a compaction.
    /// `None` when the toolbox cannot tell; an empty list when none runs. The default
    /// cannot tell.
    async fn jobs(&self, _conversation_id: ConversationId) -> Option<Vec<String>> {
        None
    }

    /// Moves the conversation's hidden shell to `dir`, because the user went there
    /// since the previous prompt. A shell that cannot move (a command still runs in
    /// it) stays where it is, and [`shell_cwd`](Toolbox::shell_cwd) then says where.
    /// The default does nothing.
    async fn move_shell(&self, _conversation_id: ConversationId, _dir: &Path) {}

    /// Stops the work of a call whose [`invoke`](Toolbox::invoke) future the turn
    /// dropped because the user interrupted it, such as a command still running in the
    /// hidden shell. The default does nothing.
    async fn cancel(&self, _call: &CallContext) {}

    /// Moves `changes` of the contained call `call` back from quarantine, after the
    /// user chose to keep them. Only a change that the launcher quarantined is moved.
    /// `Err` is the text the model reads when they stay in quarantine. The default
    /// keeps no quarantine, so it moves nothing.
    async fn restore_quarantine(
        &self,
        _call: &CallContext,
        _changes: &[SurfaceChange],
    ) -> Result<(), String> {
        Err("this toolbox keeps no quarantine".to_owned())
    }

    /// The files that the turn `turn_id` changed through the sandbox's launcher and
    /// that run code later outside the sandbox, such as `build.rs` or a git setting,
    /// for the report at the end of the turn. The turn records them as
    /// `turn_surface_report` before its terminal event. The default reports none.
    async fn turn_report(
        &self,
        _conversation_id: ConversationId,
        _turn_id: TurnId,
    ) -> Vec<ReportedFile> {
        Vec::new()
    }

    /// The files that the turn `turn_id` changed, from the toolbox's snapshots before its
    /// first call that can write and at its end, for `turn_completed`. The turn asks
    /// once, before its terminal event, whatever the ending, so the toolbox can keep
    /// the turn's last snapshot. The default takes no snapshot.
    async fn turn_changes(
        &self,
        _conversation_id: ConversationId,
        _turn_id: TurnId,
    ) -> Option<FileChanges> {
        None
    }
}

/// One tool call, as the turn hands it to the [`Toolbox`].
///
/// `Debug` shows the input's size, not its text: a command line can carry a secret.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolCall {
    /// The tool's name, as the model wrote it.
    pub name: String,
    /// The input, as the model wrote it.
    pub input: Value,
    /// Where the call runs.
    pub context: CallContext,
}

impl ToolCall {
    /// A call of the tool `name` with `input` in `context`.
    pub fn new(name: impl Into<String>, input: Value, context: CallContext) -> Self {
        ToolCall { name: name.into(), input, context }
    }
}

impl fmt::Debug for ToolCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let input_bytes = self.input.to_string().len();
        f.debug_struct("ToolCall")
            .field("name", &self.name)
            .field("input_bytes", &input_bytes)
            .field("context", &self.context)
            .finish()
    }
}

/// Where one tool call runs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CallContext {
    /// The conversation; it also names the hidden shell a command runs in.
    pub conversation_id: ConversationId,
    /// The turn.
    pub turn_id: TurnId,
    /// The call.
    pub call_id: CallId,
    /// The user's working directory when the prompt was sent: relative paths resolve
    /// against it, and a new hidden shell starts in it.
    pub cwd: PathBuf,
    /// Where the conversation's hidden shell was when the call was judged, if one
    /// runs: a relative path in a command resolves against it, not against `cwd`.
    pub shell_cwd: Option<PathBuf>,
    /// The conversation's `$SCRATCH` directory.
    pub scratch: PathBuf,
    /// The turn's scope.
    pub scope: Scope,
    /// The surface the turn came from.
    pub origin: Origin,
    /// True when the user approved the call although, or because, it may wait for input
    /// at the terminal, such as a `sudo` password: such a call may run past the model's
    /// timeout while someone who can answer follows it. Set by the check point once the
    /// approval came; false while the call is judged.
    pub approved_interactive: bool,
    /// How a `shell` call runs: [`Launch::Direct`] (typed into the hidden shell) in
    /// `manual` and `cautious` and for the other tools, [`Launch::Contained`] with its
    /// grants in the `auto` sandbox, [`Launch::Unsandboxed`] in the exit child. Set by
    /// the check point once the call may run; `Direct` while the call is judged.
    pub launch: Launch,
    /// The exits of the call that the user approved, in the order the engine found
    /// them; empty for a call without one. The toolbox reads how a write grant binds
    /// its target ([`ExitNeed::bind`]): a target that efrd makes first is made before
    /// the launch.
    pub exits: Vec<ExitNeed>,
    /// True when the turn runs in `auto` (its effective mode, after a fallback and the
    /// cap of its origin). Only `auto` reads the facts of a shell call, so the toolbox
    /// collects them only then.
    pub auto: bool,
}

impl CallContext {
    /// The context of the call `call_id` of the turn `turn_id`, in the machine scope
    /// from the shell origin until [`with_scope`](Self::with_scope) and
    /// [`with_origin`](Self::with_origin) say otherwise.
    pub fn new(
        conversation_id: ConversationId,
        turn_id: TurnId,
        call_id: CallId,
        cwd: impl Into<PathBuf>,
        scratch: impl Into<PathBuf>,
    ) -> Self {
        CallContext {
            conversation_id,
            turn_id,
            call_id,
            cwd: cwd.into(),
            shell_cwd: None,
            scratch: scratch.into(),
            scope: Scope::Machine,
            origin: Origin::Shell,
            approved_interactive: false,
            launch: Launch::Direct,
            exits: Vec::new(),
            auto: false,
        }
    }

    /// Sets whether the turn runs in `auto`.
    #[must_use]
    pub fn with_auto(mut self, auto: bool) -> Self {
        self.auto = auto;
        self
    }

    /// Sets whether the user approved the call as one that may wait for input at the
    /// terminal.
    #[must_use]
    pub fn with_approved_interactive(mut self, approved: bool) -> Self {
        self.approved_interactive = approved;
        self
    }

    /// Sets how the call runs.
    #[must_use]
    pub fn with_launch(mut self, launch: Launch) -> Self {
        self.launch = launch;
        self
    }

    /// Sets the exits of the call that the user approved.
    #[must_use]
    pub fn with_exits(mut self, exits: Vec<ExitNeed>) -> Self {
        self.exits = exits;
        self
    }

    /// Sets where the conversation's hidden shell is.
    #[must_use]
    pub fn with_shell_cwd(mut self, shell_cwd: Option<PathBuf>) -> Self {
        self.shell_cwd = shell_cwd;
        self
    }

    /// Sets the turn's scope.
    #[must_use]
    pub fn with_scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }

    /// Sets the surface the turn came from.
    #[must_use]
    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }
}

/// What a tool call returns to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolOutcome {
    /// The text the model sees, already cut to size by the tool.
    pub output: String,
    /// True when `output` was cut.
    pub truncated: bool,
    /// True when the call failed: an error for the model, or a command that did not
    /// exit with 0.
    pub is_error: bool,
    /// The exit status, for a tool that runs a command.
    pub exit_code: Option<i32>,
    /// What a call through the sandbox's launcher did to the state around it: names
    /// only, never values. The turn records it with the call's completion, and asks the
    /// user about each change that the launcher moved to quarantine.
    pub sandbox: Option<SandboxSummary>,
    /// The files that the call changed, for `tool_call_completed`.
    pub changes: Option<FileChanges>,
    /// The unified diff of a file tool's write, for `tool_call_completed`.
    pub diff: Option<String>,
}

impl ToolOutcome {
    /// A successful outcome.
    pub fn ok(output: impl Into<String>) -> Self {
        ToolOutcome {
            output: output.into(),
            truncated: false,
            is_error: false,
            exit_code: None,
            sandbox: None,
            changes: None,
            diff: None,
        }
    }

    /// A failed outcome: the model reads `output` and decides what to do next.
    pub fn error(output: impl Into<String>) -> Self {
        ToolOutcome { is_error: true, ..ToolOutcome::ok(output) }
    }

    /// Marks the output as cut or not.
    #[must_use]
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    /// Sets the exit status.
    #[must_use]
    pub fn with_exit_code(mut self, exit_code: Option<i32>) -> Self {
        self.exit_code = exit_code;
        self
    }

    /// Sets what a call through the sandbox's launcher reported.
    #[must_use]
    pub fn with_sandbox(mut self, sandbox: Option<SandboxSummary>) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// Sets the files that the call changed.
    #[must_use]
    pub fn with_changes(mut self, changes: Option<FileChanges>) -> Self {
        self.changes = changes;
        self
    }

    /// Sets the unified diff of a file tool's write.
    #[must_use]
    pub fn with_diff(mut self, diff: Option<String>) -> Self {
        self.diff = diff;
        self
    }
}

/// Hears a call's output while it runs; the turn turns it into coalesced
/// `tool_call_output_updated` events, and each change of whether the call's command
/// waits for input into a `tool_call_input_changed` event.
pub trait OutputSink: Send {
    /// The end of the output so far and the size of all of it in bytes.
    fn update(&mut self, tail: &str, bytes: u64);

    /// The call's command started or stopped waiting for input. Each change is
    /// recorded, none is coalesced. `looks_secret` marks a visible wait whose prompt
    /// reads like a password prompt behind a relay. Ignored by default.
    fn input_changed(&mut self, _wait: InputWait, _looks_secret: bool) {}
}

impl<F: FnMut(&str, u64) + Send> OutputSink for F {
    fn update(&mut self, tail: &str, bytes: u64) {
        self(tail, bytes);
    }
}

#[cfg(test)]
mod tests;
