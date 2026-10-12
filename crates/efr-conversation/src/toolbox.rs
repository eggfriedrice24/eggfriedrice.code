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
    CallId, ConversationId, FileChanges, InputWait, JobInfo, Launch, Origin, ReportedFile,
    SandboxSummary, Scope, SnapshotPhase, SurfaceChange, TurnId, TurnUndo, TurnUndoInfo,
    TurnUndoResult, UndoId, UndoReason, UndoRefused,
};
use efr_provider::{EditTool, ToolDefinition};
use serde_json::Value;

use crate::UndoBusyReason;

/// The tools a conversation offers the model.
///
/// The turn asks [`requirements`](Toolbox::requirements) first and hands the answer to
/// the permission engine; only an allowed or approved call reaches
/// [`invoke`](Toolbox::invoke). A toolbox never decides a permission itself.
#[async_trait]
pub trait Toolbox: Send + Sync + fmt::Debug {
    /// The tools for the provider request, in a stable order, so that the request stays
    /// the same from one model call to the next. Of the two tools that change files,
    /// `apply_patch` and `edit`, it offers only the one that `edit` names: the tool that
    /// the request's model knows ([`efr_provider::ModelInfo::edit_tool`]).
    fn definitions(&self, edit: EditTool) -> Vec<ToolDefinition>;

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

    /// The child processes of the conversation's hidden shell, which a call left
    /// running in the shell's background, one line each (its command line), for the
    /// fresh context block after a compaction. They are not background jobs of efr
    /// ([`background_jobs`](Toolbox::background_jobs)). `None` when the toolbox cannot
    /// tell; an empty list when none runs. The default cannot tell.
    async fn shell_children(&self, _conversation_id: ConversationId) -> Option<Vec<String>> {
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

    /// Deletes `paths` in the user's real tool caches, after the classifier or the user
    /// allowed it: the paths that the contained call `call` deleted in its private cache
    /// layer (`SandboxSummary::cache_deletes`). The toolbox checks each path again
    /// against its own list of caches and pins. `Err` is the text the model reads when
    /// nothing was deleted. The default keeps no caches, so it deletes nothing.
    async fn delete_cache_paths(
        &self,
        _call: &CallContext,
        _paths: &[PathBuf],
    ) -> Result<CacheDeleteOutcome, String> {
        Err("this toolbox keeps no tool caches, so the deletes stayed private".to_owned())
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

    /// What the turn `turn_id` changed, from the toolbox's snapshots before its first
    /// call that can write and at its end: the files for `turn_completed`, what
    /// `efr undo` can take back, and the snapshots for `snapshot_taken`. The turn asks
    /// once, before its terminal event, whatever the ending, so the toolbox can keep
    /// the turn's last snapshot. The default takes no snapshot.
    async fn turn_changes(
        &self,
        _conversation_id: ConversationId,
        _turn_id: TurnId,
    ) -> TurnChanges {
        TurnChanges::default()
    }

    /// Takes back the file and ref changes of one turn of `request.candidates`, or with
    /// `dry_run` only says what it would do (`turn.undo`, phase B). The actor calls it
    /// only while no turn, compaction or other undo of the conversation runs. `Err`
    /// says why nothing changed. The default has no undo, so it refuses with
    /// [`UndoRefused::Off`].
    async fn undo(&self, _request: UndoRequest) -> Result<TurnUndoResult, UndoRefused> {
        Err(UndoRefused::Off)
    }

    /// True while a call of the conversation still runs after its tool answered, such
    /// as a command that went on past its timeout, so an undo must wait (phase B). The
    /// default runs no such call.
    fn call_runs(&self, _conversation_id: ConversationId) -> bool {
        false
    }

    /// The background jobs of the conversation, newest first: the ones that run and
    /// the ones that ended lately (phase C), for the fresh context block after a
    /// compaction. The default runs no job.
    async fn background_jobs(&self, _conversation_id: ConversationId) -> Vec<JobInfo> {
        Vec::new()
    }

    /// Why an undo of the turn `turn_id` must wait for a background job: a job of the
    /// conversation runs, or a job of another conversation writes in a root of the
    /// turn ([`UndoBusyReason::Job`]). `None` when no job is in the way. The default
    /// runs no job.
    async fn job_blocks_undo(
        &self,
        _conversation_id: ConversationId,
        _turn_id: TurnId,
    ) -> Option<UndoBusyReason> {
        None
    }

    /// True when the daemon-wide limit of turns that job notices start per hour lets
    /// one more start now (`jobs.max_wakes_per_hour_daemon`). The limit of each
    /// conversation is the actor's own. The default has no daemon-wide limit.
    fn wake_allowed(&self, _conversation_id: ConversationId) -> bool {
        true
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
    /// The tool with which the turn's model changes files, the one that the request
    /// offered. A call of the other one was not offered, and the toolbox refuses it.
    pub edit_tool: EditTool,
}

impl CallContext {
    /// The context of the call `call_id` of the turn `turn_id`, in the machine scope
    /// from the shell origin, for a model that changes files with `apply_patch`, until
    /// [`with_scope`](Self::with_scope), [`with_origin`](Self::with_origin) and
    /// [`with_edit_tool`](Self::with_edit_tool) say otherwise.
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
            edit_tool: EditTool::default(),
        }
    }

    /// Sets the tool with which the turn's model changes files.
    #[must_use]
    pub fn with_edit_tool(mut self, edit_tool: EditTool) -> Self {
        self.edit_tool = edit_tool;
        self
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
    /// The floor files that the call changed, each with the copy that the toolbox made
    /// before the call, for `home_file_changed` and the line at the end of the turn. A
    /// file that the call did not change has no entry.
    pub backups: Vec<FileBackup>,
    /// The background job that the call started (a `shell` call with
    /// `background: true`). The toolbox recorded its `job_started` before it answered;
    /// the actor keeps the turn's origin and settings for the job's notices.
    pub job: Option<JobInfo>,
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
            backups: Vec::new(),
            job: None,
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

    /// Sets the floor files that the call changed, with their backups.
    #[must_use]
    pub fn with_backups(mut self, backups: Vec<FileBackup>) -> Self {
        self.backups = backups;
        self
    }

    /// Sets the background job that the call started.
    #[must_use]
    pub fn with_job(mut self, job: JobInfo) -> Self {
        self.job = Some(job);
        self
    }
}

/// A floor file that a call changed, and the copy of it from before the call in efr's
/// backup store (efr's auto spec, section 10.9). From phase B the copy is in efr's home
/// store (section 10.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBackup {
    /// The file that the call changed, as the call named it, absolute.
    pub path: PathBuf,
    /// The copy from before the call, in efr's phase A backup store. Empty when the copy
    /// is in efr's home store (phase B), which has no path of its own for one file.
    pub backup: PathBuf,
    /// True when the file is a floor.
    pub floor: bool,
    /// The real file, when `path` reaches it through a link, such as
    /// `~/dotfiles/hypr/x.conf` for `~/.config/hypr/x.conf`; `None` when `path` is the
    /// real file.
    pub real: Option<PathBuf>,
    /// Why efr has no copy of the file, so `efr undo` cannot bring it back, such as a
    /// masked file (phase B). `None` when efr copied it.
    pub reason: Option<UndoReason>,
}

/// One snapshot that efr took for a turn, for the turn's `snapshot_taken` events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRecord {
    /// The root as a client shows it, or `home` for a copy of files outside the roots.
    pub root: String,
    /// Before or after.
    pub phase: SnapshotPhase,
    /// The snapshot's commit, in hex.
    pub commit: String,
    /// The files in the snapshot.
    pub files: u32,
    /// The files that the snapshot left out.
    pub skipped: u32,
    /// How long the snapshot took, in milliseconds.
    pub ms: u64,
}

/// What [`Toolbox::turn_changes`] says about a turn at its end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnChanges {
    /// The files that the turn changed; `None` when nothing changed or efr took no
    /// snapshot.
    pub changes: Option<FileChanges>,
    /// What `efr undo` can take back of the turn; `None` when the turn has no snapshot.
    pub undo: Option<TurnUndoInfo>,
    /// The snapshots of the turn: both phases of each root, and the copies of files
    /// outside the roots.
    pub snapshots: Vec<SnapshotRecord>,
}

/// A turn that an undo may take back, from the log of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoCandidate {
    /// The turn.
    pub turn_id: TurnId,
    /// The first line of the turn's prompt.
    pub title: Option<String>,
    /// When the turn ended; `None` for a turn that efrd stopped.
    pub ended: Option<jiff::Timestamp>,
    /// True when efrd stopped while the turn ran.
    pub stopped: bool,
}

/// What [`Toolbox::undo`] gets: the request of `turn.undo` with the turns of the
/// conversation and the id of the new undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRequest {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The ended turns of the conversation, newest first.
    pub candidates: Vec<UndoCandidate>,
    /// The params of `turn.undo`.
    pub params: TurnUndo,
    /// The id of the undo, from the actor's clock and generator.
    pub undo_id: UndoId,
    /// The surface that asked.
    pub origin: Origin,
}

/// What [`Toolbox::delete_cache_paths`] deleted in the real tool caches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheDeleteOutcome {
    /// The paths that it deleted.
    pub deleted: Vec<PathBuf>,
    /// The bytes that the deletes freed.
    pub freed_bytes: u64,
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
