//! The tools: the registry composed from `efr-tools`, offered to conversations through
//! the `Toolbox` trait of `efr-conversation`.
//!
//! `efr-conversation` cannot depend on `efr-tools` (the chain reaches `efr-shell`, a
//! forbidden edge), so [`DaemonToolbox`] copies between the two, field by field: specs
//! to tool definitions, tool requirements to permission requirements, results to
//! outcomes. It never decides a permission; the conversation's check point does.
//!
//! Next to the registry it offers the settings tool ([`SettingsTool`], in
//! `tools/settings_tool.rs`), which needs `efr-config` and the daemon's settings and so
//! cannot live in `efr-tools`. Its changes declare a settings change, which the engine
//! always asks about.
//!
//! It also decides who can answer a command that waits for hidden input, such as a
//! password: a call's sink says yes while the conversation has a live subscription
//! with `answers_input` (see `connections.rs`), so a command that asks for a password
//! while nobody who can type it follows the turn is stopped at once instead of waiting
//! for its timeout. The same subscriptions keep a call that the user approved because it
//! may wait for input running past the model's timeout, up to
//! `shell.interactive_timeout_minutes`, while one of them follows the conversation.
//!
//! Before any call runs, it copies the targets of the call's approved exits
//! ([`ExitNeed::copies`](efr_permissions::ExitNeed::copies)) into the home store of
//! `efr-snapshot` (`home.rs`); a copy that fails stops the call, and after the call
//! each changed file comes back as a `FileBackup`. It also applies the cache deletes
//! that the turn allowed (`tools/cache_deletes.rs`), after its own check of each path,
//! with a copy within the same limits first.
//!
//! In `auto` it answers what the engine asks efr to hold before a call
//! ([`FactRequest::covers`](efr_permissions::FactRequest)): a snapshot ask from a
//! snapshot taken now under the plan lock, a home ask from the sizes and the masks
//! (plan PB15). Under the call's own plan lock, after the call's snapshot, it asks again
//! each snapshot ask that was covered, and a call whose answer changed does not start.
//! [`Toolbox::undo`] is in `tools/undo.rs`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use efr_config::{Settings, SudoCache, WriteProjects};
use efr_conversation::{
    CacheDeleteOutcome, CallContext, FileBackup, OutputSink, SnapshotRecord, ToolCall, ToolOutcome,
    Toolbox, TurnChanges, UndoRequest,
};
use efr_permissions::{CoverAsk, Coverage, Engine, Requirements};
use efr_protocol::{
    CallId, ConversationId, FileChanges, InputWait, ReportedFile, SandboxSummary, SnapshotPhase,
    SurfaceChange, TurnId, TurnUndoResult, UndoRefused,
};
use efr_provider::{ToolDefinition, ToolGrammar as ProviderGrammar};
use efr_scope::Home;
use efr_shell::{ShellError, ShellSessions};
use efr_snapshot::HomeState;
use efr_stdx::time::{Clock, Stopwatch};
use efr_tools::{
    AccessMode, ApplyPatchTool, CallIds, EditTool, JournalEntry, ReadFileTool, ShellTool,
    ToolContext, ToolError, ToolOutputSink, ToolRegistry, ToolRequirements, ToolResult, ToolSpec,
    WriteFileTool, WriteJournal, WrittenKind, not_ready_message,
};
use serde_json::Value;
use tokio::sync::watch;

use crate::DaemonError;
use crate::connections::Connections;
use crate::home::state::{self, StateCopy};
use crate::home::{self, HomeLimits};
use crate::sandbox::{PrepareInput, SandboxService, facts, lock, quarantine, units};
use crate::trusted_programs::TrustedPrograms;

mod cache_deletes;
mod jobs;
mod settings_tool;
mod snapshot;
mod undo;

pub(crate) use settings_tool::SettingsTool;
pub(crate) use snapshot::CallSnapshots;

/// What the model reads for a call that must run through the sandbox's launcher while
/// this toolbox has no sandbox; it never runs in the hidden shell instead.
pub(crate) const SANDBOX_NOT_STARTED: &str =
    "[the sandbox could not start: this efrd does not run the sandbox. The command did not run.]";

/// What the model reads for a call whose files efr's snapshot held when the call was
/// judged, and no longer holds under the plan lock: it did not run, and the next
/// decision sees the new facts.
pub(crate) const COVERAGE_CHANGED: &str = "[efr: the files of this command changed after efr \
     checked that its snapshot holds them, so efr undo could not bring them back. The command \
     did not run; run it again.]";

/// How many turns of `auto` efrd keeps in mind for `efr undo --list`; a turn id is a
/// version 7 UUID, so the smallest is the oldest.
const MAX_AUTO_TURNS: usize = 4096;

/// What the model reads for a call whose home files efr could no longer copy.
pub(crate) const HOME_COPY_CHANGED: &str = "[efr: a file outside the project changed after \
     efr checked that it can copy it, so efr undo could not bring it back. The call did not \
     run; run it again.]";

/// The tools that write files in the daemon, outside the hidden shell: they take the
/// plan lock of their projects and the turn's first snapshot before they write.
const FILE_WRITERS: [&str; 3] = [WriteFileTool::NAME, ApplyPatchTool::NAME, EditTool::NAME];

/// The tools that change files, of which a request offers the model one: the one that
/// the model knows ([`efr_provider::EditTool`]).
const EDIT_TOOLS: [&str; 2] = [ApplyPatchTool::NAME, EditTool::NAME];

/// The name of the tool with which a model that knows `edit` changes files.
fn edit_tool_name(edit: efr_provider::EditTool) -> &'static str {
    match edit {
        efr_provider::EditTool::Replace => EditTool::NAME,
        // NOTE: `apply_patch` is the default; a kind that this daemon does not know yet
        // gets it too.
        _ => ApplyPatchTool::NAME,
    }
}

/// True when a request for a model that knows `edit` offers the tool `name`: every tool
/// but the edit tool that the model does not know.
fn offered(name: &str, edit: efr_provider::EditTool) -> bool {
    !EDIT_TOOLS.contains(&name) || name == edit_tool_name(edit)
}

/// The registry: the shell, `read_file`, `write_file`, `apply_patch` and `edit`. A
/// request offers a model `apply_patch` or `edit`, never both. A provider sends
/// `apply_patch` in its freeform form to a model that takes it, and in its function
/// form to every other model.
pub(crate) fn registry(shells: &ShellSessions) -> Result<ToolRegistry, DaemonError> {
    let mut registry = ToolRegistry::new();
    let shell = ShellTool::new(Arc::new(shells.clone()));
    registry.register(Arc::new(shell)).map_err(|source| DaemonError::Tool { source })?;
    registry
        .register(Arc::new(ReadFileTool::new()))
        .map_err(|source| DaemonError::Tool { source })?;
    registry
        .register(Arc::new(WriteFileTool::new()))
        .map_err(|source| DaemonError::Tool { source })?;
    // NOTE: a delete or a move of a patch declares itself destructive, and the engine
    // asks about it in every mode (`efr_permissions::Requirements::destructive`).
    registry.register(Arc::new(ApplyPatchTool)).map_err(|source| DaemonError::Tool { source })?;
    registry.register(Arc::new(EditTool)).map_err(|source| DaemonError::Tool { source })?;
    Ok(registry)
}

/// The registry and the settings tool as a conversation's `Toolbox`.
#[derive(Debug)]
pub(crate) struct DaemonToolbox {
    registry: ToolRegistry,
    /// The settings tool, offered after the registry's tools.
    settings_tool: SettingsTool,
    shells: ShellSessions,
    home: Home,
    clock: Arc<dyn Clock>,
    journal: Arc<dyn WriteJournal>,
    /// The live subscriptions, which say whether a person can answer a waiting command.
    connections: Arc<Connections>,
    /// The daemon's settings; each call reads `shell.sudo_cache` from the latest.
    settings: watch::Receiver<Arc<Settings>>,
    /// The `auto` sandbox and the engine whose locations a call's facts and spec read;
    /// `None` refuses every call through the launcher.
    sandbox: Option<(SandboxService, watch::Receiver<Arc<Engine>>)>,
    /// The snapshots of the calls that can write, and the home store; `None` takes
    /// none.
    snapshots: Option<CallSnapshots>,
    /// The undo trash, `$XDG_DATA_HOME/efr/undo-trash`; `None` keeps no trash, so undo
    /// writes over nothing.
    undo_trash: Option<PathBuf>,
    /// efr's own roots, which undo never writes in the home directory.
    efr_roots: Vec<PathBuf>,
    /// What the facts of each call said efr covers, until the call runs: the home asks
    /// that the copy must cover, and the snapshot asks that are asked again under the
    /// call's plan lock (plan PB15).
    covered: Arc<Mutex<HashMap<CallId, Answers>>>,
    /// What the home copies of each running turn found, for its end.
    home_turns: Arc<Mutex<HashMap<TurnId, HomeTally>>>,
    /// The turns that ran in `auto` since efrd started.
    auto_turns: Arc<Mutex<HashSet<TurnId>>>,
    /// The `crontab` and `loginctl` that efrd runs for the state copies of cron and
    /// linger (`trusted_programs.rs`).
    programs: TrustedPrograms,
}

/// What the facts of one call said efr covers: its turn and each ask with its answer.
type Answers = (TurnId, Vec<(CoverAsk, Coverage)>);

/// What the home copies of one turn found.
#[derive(Debug, Clone, Default)]
struct HomeTally {
    /// Each home file that a call changed and a copy covers.
    changed: Vec<PathBuf>,
    /// Each one that no copy covers, as the end of the turn names it.
    not_covered: Vec<String>,
    /// One record per copy.
    records: Vec<SnapshotRecord>,
    /// The targets of each copy whose "after" is not written yet, by call: a copy that
    /// still waits at the end of the turn did not finish (its call was interrupted or
    /// its "after" failed), so undo leaves its paths alone (plan PB12).
    pending: Vec<(CallId, Vec<String>)>,
}

/// One home copy of a call, between the copy before it and the copy after it.
struct Copying {
    copy: efr_snapshot::HomeCopy,
    before: home::Prepared,
    masks: Vec<PathBuf>,
    limits: HomeLimits,
}

impl DaemonToolbox {
    pub(crate) fn new(
        registry: ToolRegistry,
        shells: ShellSessions,
        home: Home,
        clock: Arc<dyn Clock>,
        connections: Arc<Connections>,
        settings: watch::Receiver<Arc<Settings>>,
        settings_tool: SettingsTool,
    ) -> Self {
        DaemonToolbox {
            registry,
            settings_tool,
            shells,
            home,
            clock,
            journal: Arc::new(LogJournal),
            connections,
            settings,
            sandbox: None,
            snapshots: None,
            undo_trash: None,
            efr_roots: Vec::new(),
            covered: Arc::default(),
            home_turns: Arc::default(),
            auto_turns: Arc::default(),
            programs: TrustedPrograms::none(),
        }
    }

    /// Takes snapshots before and after the calls that can write, with `snapshots`.
    pub(crate) fn with_snapshots(mut self, snapshots: CallSnapshots) -> Self {
        self.snapshots = Some(snapshots);
        self
    }

    /// Keeps the undo trash in `dir`, `$XDG_DATA_HOME/efr/undo-trash`.
    pub(crate) fn with_undo_trash(mut self, dir: PathBuf) -> Self {
        self.undo_trash = Some(dir);
        self
    }

    /// Runs `programs` for the state copies of cron and linger.
    pub(crate) fn with_programs(mut self, programs: TrustedPrograms) -> Self {
        self.programs = programs;
        self
    }

    /// Never lets undo write in efr's own roots `roots` (`$C`, `$D`, `$S`, `$R`).
    pub(crate) fn with_efr_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.efr_roots = roots;
        self
    }

    /// The snapshots before a `shell` call of `call`; `named` adds the projects that a
    /// line of `auto` names.
    async fn snapshot_before(
        &self,
        call: &CallContext,
        named: &[PathBuf],
    ) -> Option<(efr_snapshot::CallSnapshot, efr_snapshot::Limits)> {
        self.snapshots.as_ref()?.before_call(call, named).await
    }

    /// What a `shell` call changed since [`snapshot_before`](Self::snapshot_before).
    async fn snapshot_after(
        &self,
        before: Option<(efr_snapshot::CallSnapshot, efr_snapshot::Limits)>,
    ) -> Option<FileChanges> {
        self.snapshots.as_ref()?.after_call(before).await
    }

    /// Before a `write_file`, `apply_patch` or `edit` call: the turn's first snapshot of
    /// each root that holds one of its targets.
    async fn snapshot_before_write(&self, call: &ToolCall) {
        let Some(snapshots) = &self.snapshots else { return };
        let context = self.context(&call.context);
        let Ok(declared) = self.registry.requirements(&call.name, &context, &call.input) else {
            return;
        };
        let projects = match &self.sandbox {
            Some((sandbox, _)) => sandbox.projects().await,
            None => Vec::new(),
        };
        for access in &declared.paths {
            snapshots.before_write(&call.context, &access.path, &projects).await;
        }
    }

    /// Runs the calls through the launcher with `sandbox`, and reads the latest engine
    /// from `engine` for their facts and specs.
    pub(crate) fn with_sandbox(
        mut self,
        sandbox: SandboxService,
        engine: watch::Receiver<Arc<Engine>>,
    ) -> Self {
        self.sandbox = Some((sandbox, engine));
        self
    }

    fn context(&self, call: &CallContext) -> ToolContext {
        let ids = CallIds {
            conversation_id: call.conversation_id,
            turn_id: call.turn_id,
            call_id: call.call_id,
        };
        ToolContext::new(
            ids,
            &call.cwd,
            &call.scratch,
            self.home.clone(),
            Arc::clone(&self.clock),
            Arc::clone(&self.journal),
        )
        .with_scope(call.scope.clone())
        .with_origin(call.origin)
        .with_shell_cwd(call.shell_cwd.clone())
        .with_forget_credentials(self.forgets_credentials())
        .with_interactive_limit(self.interactive_limit(call))
    }

    /// How long a call that the user approved because it may wait for input may run
    /// while someone who can answer follows it, from the latest settings, so a change
    /// reaches the next call; `None` for any other call, which keeps the model's
    /// timeout.
    fn interactive_limit(&self, call: &CallContext) -> Option<Duration> {
        let minutes = self.settings.borrow().shell.interactive_timeout_minutes;
        call.approved_interactive.then(|| Duration::from_secs(minutes.saturating_mul(60)))
    }

    /// True when the latest settings make the hidden shell forget sudo's credentials
    /// after each call, read at each call so a change reaches the next one.
    fn forgets_credentials(&self) -> bool {
        self.settings.borrow().shell.sudo_cache == SudoCache::PerCall
    }
}

impl DaemonToolbox {
    /// The facts that the engine reads in `auto`: about the files and programs of a
    /// shell call, and about the targets of a `write_file`, `apply_patch` or `edit`
    /// call; `None` for another tool or without a sandbox.
    async fn facts(
        &self,
        call: &ToolCall,
        declared: &ToolRequirements,
    ) -> Option<efr_permissions::CallFacts> {
        let (sandbox, engine) = self.sandbox.as_ref()?;
        let writes: Vec<PathBuf> = declared
            .paths
            .iter()
            .filter(|access| !matches!(access.mode, AccessMode::Read | AccessMode::ReadTree))
            .map(|access| access.path.clone())
            .collect();
        let Some(command) = declared.command.as_deref() else {
            if !FILE_WRITERS.contains(&call.name.as_str()) {
                return None;
            }
            // NOTE: a file tool has no line, only the paths it writes. Their kinds and
            // those of their parents tell a file and a new file from a directory, so a
            // floor file edit can go to the classifier and a directory never does.
            let engine = Arc::clone(&engine.borrow());
            let requirements = permission_requirements(declared.clone());
            let request =
                efr_permissions::exits::tool_fact_requests(&requirements, engine.locations());
            let masks = self.masks();
            let input = facts::FactInput {
                request: &request,
                writes: &writes,
                command_dir: None,
                shell_path: "",
                rebuildable: &[],
                turn_start: self.clock.now(),
                masks: &masks,
            };
            let mut facts = facts::collect(&input, sandbox.git(), &self.home).await;
            // NOTE: the real file behind a dotfile link is a second write; the link
            // tells the engine to give it the names of the link (`~/.zshrc`).
            facts.links =
                declared.links.iter().filter(|(_, real)| writes.contains(real)).cloned().collect();
            // NOTE: a file tool writes in every registered project, as its check under
            // the lock does.
            let projects = sandbox.projects().await;
            facts.covered = self.cover_facts(&call.context, &request.covers, projects).await;
            return Some(facts);
        };
        let engine = Arc::clone(&engine.borrow());
        let command_dir = declared.command_dir.as_deref();
        let request =
            efr_permissions::exits::fact_requests(command, command_dir, engine.locations())
                .with_needs(declared.needs.as_ref(), command_dir, engine.locations());
        let rebuildable = self.settings.borrow().sandbox.rebuildable.clone();
        let turn_start = sandbox.turns().started(call.context.turn_id, self.clock.now());
        let shell_path = sandbox.shell_path(call.context.conversation_id);
        let masks = self.masks();
        let input = facts::FactInput {
            request: &request,
            writes: &writes,
            command_dir,
            shell_path: &shell_path,
            rebuildable: &rebuildable,
            turn_start,
            masks: &masks,
        };
        let mut facts = facts::collect(&input, sandbox.git(), &self.home).await;
        let projects = cover_projects(
            sandbox.projects().await,
            self.settings.borrow().sandbox.write_projects,
            self.home.path(),
        );
        facts.covered = self.cover_facts(&call.context, &request.covers, projects).await;
        Some(facts)
    }

    /// The answer to each ask of `asks` for the call `call`, which the call keeps until
    /// it runs. A snapshot ask is answered from a snapshot of its root taken now, under
    /// the root's plan lock, so no call writes in between (the turn's `pre` when the
    /// turn has none yet); a home ask outside the roots from its sizes and the masks;
    /// a home ask inside a root gets no answer, since the snapshot covers it. `projects`
    /// are the registered projects besides the turn's own that the call may write: the
    /// same that its check under the lock reads.
    async fn cover_facts(
        &self,
        call: &CallContext,
        asks: &[CoverAsk],
        projects: Vec<PathBuf>,
    ) -> Vec<(CoverAsk, Coverage)> {
        if asks.is_empty() {
            return Vec::new();
        }
        let mut answers: Vec<(CoverAsk, Coverage)> = Vec::new();
        match &self.snapshots {
            Some(snapshots) => {
                let roots = snapshots.cover_roots(call, asks, &projects);
                let guard = match (&self.sandbox, roots.is_empty()) {
                    (Some((sandbox, _)), false) => Some(sandbox.locks().lock(&roots).await),
                    _ => None,
                };
                answers.extend(snapshots.covers(call, asks, &projects).await);
                drop(guard);
            }
            None => answers.extend(
                asks.iter()
                    .filter(|ask| !home_side(ask))
                    .map(|ask| (ask.clone(), Coverage::NoSnapshot)),
            ),
        }
        // NOTE: the crontab and linger have no file; efrd copies them with the programs
        // that set them, when it found those (plan PB20). A unit change writes links in
        // the unit config dir, which efrd copies whole (PB34).
        let kept = self.snapshots.is_some() && self.settings.borrow().snapshot.enabled;
        for ask in asks.iter().filter(|ask| StateCopy::of(ask).is_some()) {
            let coverage =
                if kept { state::coverage(&self.programs, ask) } else { Coverage::NoSnapshot };
            answers.push((ask.clone(), coverage));
        }
        let unit_asks: Vec<String> = asks
            .iter()
            .filter_map(|ask| match ask {
                CoverAsk::UnitLinks(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        if !unit_asks.is_empty() {
            let (masks, home) = (self.masks(), self.home.path().to_path_buf());
            let limits = HomeLimits::of(&self.settings.borrow());
            let answered = tokio::task::spawn_blocking(move || {
                unit_asks
                    .into_iter()
                    .map(|name| {
                        let coverage = if kept {
                            units::coverage(&name, &home, &masks, limits)
                        } else {
                            Coverage::NoSnapshot
                        };
                        (CoverAsk::UnitLinks(name), coverage)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            answers.extend(answered);
        }
        let mut inside: Vec<PathBuf> = projects;
        inside.push(call.scratch.clone());
        let home_asks: Vec<CoverAsk> = asks
            .iter()
            .filter(|ask| match ask {
                CoverAsk::Home(path) | CoverAsk::HomeDir(path) => {
                    !inside.iter().any(|root| path.starts_with(root))
                }
                _ => false,
            })
            .cloned()
            .collect();
        if !home_asks.is_empty() {
            let asked: Vec<PathBuf> = home_asks
                .iter()
                .filter_map(|ask| match ask {
                    CoverAsk::Home(path) | CoverAsk::HomeDir(path) => Some(path.clone()),
                    _ => None,
                })
                .collect();
            let masks = self.copy_masks(&asked).await;
            let settings = Arc::clone(&self.settings.borrow());
            let kept = self.snapshots.is_some() && settings.snapshot.enabled;
            let limits = HomeLimits::of(&settings);
            let answered = tokio::task::spawn_blocking(move || {
                home_asks
                    .into_iter()
                    .map(|ask| {
                        let coverage = if kept {
                            home::coverage(&ask, &masks, limits)
                        } else {
                            Coverage::NoSnapshot
                        };
                        (ask, coverage)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            answers.extend(answered);
        }
        let mut covered = self.covered.lock().unwrap_or_else(PoisonError::into_inner);
        covered.insert(call.call_id, (call.turn_id, answers.clone()));
        answers
    }

    /// The snapshot asks of the call `call` that its facts said are covered, taken
    /// from the record of [`cover_facts`](Self::cover_facts).
    fn covered_root_asks(&self, call: &CallContext) -> Vec<CoverAsk> {
        let mut covered = self.covered.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((_, answers)) = covered.get_mut(&call.call_id) else { return Vec::new() };
        let (roots, rest): (Vec<_>, Vec<_>) =
            std::mem::take(answers).into_iter().partition(|(ask, _)| !home_side(ask));
        *answers = rest;
        roots
            .into_iter()
            .filter(|(_, coverage)| coverage.is_covered())
            .map(|(ask, _)| ask)
            .collect()
    }

    /// The paths of the snapshot asks of the call `call` that its facts said are
    /// covered, without taking them from the record of
    /// [`cover_facts`](Self::cover_facts).
    fn covered_root_paths(&self, call: &CallContext) -> Vec<PathBuf> {
        let covered = self.covered.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((_, answers)) = covered.get(&call.call_id) else { return Vec::new() };
        answers
            .iter()
            .filter(|(_, coverage)| coverage.is_covered())
            .filter_map(|(ask, _)| match ask {
                CoverAsk::Path(path)
                | CoverAsk::Tracked(path)
                | CoverAsk::Untracked(path)
                | CoverAsk::Refs(path)
                | CoverAsk::Stash(path) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    /// Asks again, under the call's plan lock and after its snapshot, each snapshot ask
    /// that the call's facts said is covered (plan PB15). `Some` is what the model reads
    /// when one is no longer covered: the call does not start.
    async fn check_covered(&self, call: &CallContext, projects: &[PathBuf]) -> Option<String> {
        let asks = self.covered_root_asks(call);
        let snapshots = self.snapshots.as_ref()?;
        if asks.is_empty() {
            return None;
        }
        let answers = snapshots.covers(call, &asks, projects).await;
        let lost: Vec<&CoverAsk> = answers
            .iter()
            .filter(|(_, coverage)| !coverage.is_covered())
            .map(|(ask, _)| ask)
            .collect();
        if lost.is_empty() {
            return None;
        }
        tracing::info!(call_id = %call.call_id, ?lost, "a covered line is no longer covered under the plan lock");
        Some(COVERAGE_CHANGED.to_owned())
    }

    /// Takes the plan lock of the projects that a `write_file`, `apply_patch` or `edit`
    /// call writes, for the write; `None` for another tool or without a sandbox.
    async fn lock_write(&self, call: &ToolCall) -> Option<lock::PlanGuard> {
        let (sandbox, _) = self.sandbox.as_ref()?;
        if !FILE_WRITERS.contains(&call.name.as_str()) {
            return None;
        }
        let context = self.context(&call.context);
        let declared = self.registry.requirements(&call.name, &context, &call.input).ok()?;
        let projects = sandbox.projects().await;
        let roots: Vec<PathBuf> = projects
            .into_iter()
            .filter(|root| declared.paths.iter().any(|access| access.path.starts_with(root)))
            .collect();
        Some(sandbox.locks().lock(&roots).await)
    }

    /// Runs a shell call through the sandbox's launcher: prepares its call dir under the
    /// plan lock, which it releases once the launcher has opened every bind source,
    /// runs the wrapper line, and checks the sandbox again when it could not start.
    async fn invoke_sandboxed(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome {
        let Some((sandbox, engine)) = &self.sandbox else {
            return ToolOutcome::error(SANDBOX_NOT_STARTED);
        };
        if call.name != ShellTool::NAME {
            return ToolOutcome::error(SANDBOX_NOT_STARTED);
        }
        let settings = Arc::clone(&self.settings.borrow());
        let engine = Arc::clone(&engine.borrow());
        let context = self.context(&call.context);
        let mut named_paths: Vec<PathBuf> = Vec::new();
        let mut timeout = None;
        if let Ok(declared) = self.registry.requirements(&call.name, &context, &call.input) {
            named_paths.extend(declared.paths.iter().map(|access| access.path.clone()));
            named_paths.extend(declared.command_dir);
            timeout = declared.timeout;
        }
        // NOTE: a covered target names its root too, such as the other project of
        // `dd of=~/p/app/db.sqlite`: the plan locks it, takes its snapshot and lets the
        // line write there, and the check under the lock finds the root again.
        named_paths.extend(self.covered_root_paths(&call.context));
        // NOTE: the plan lock blocks every write and every plan in the call's projects,
        // also those of other conversations. A call that queues behind a command still
        // running in this shell (a dev server left at its timeout) waits here, before it
        // takes the lock, so it blocks nobody while it waits.
        let watch = Stopwatch::start();
        let free = match timeout {
            Some(timeout) => self.shells.until_free(call.context.conversation_id, timeout).await,
            None => Ok(()),
        };
        tracing::debug!(phase = "shell_free_wait", elapsed_ms = %watch, "phase=shell_free_wait elapsed_ms={}", watch);
        if let Some(timeout) = timeout
            && let Err(error) = free
        {
            let fresh = matches!(error, ShellError::NotReady { fresh: true, .. });
            return ToolOutcome::error(not_ready_message(fresh, timeout));
        }
        let input = PrepareInput { settings: &settings, engine: &engine, named_paths };
        let watch = Stopwatch::start();
        let prepared = sandbox.prepare(&call.context, &input).await;
        tracing::debug!(phase = "sandbox_prepare", elapsed_ms = %watch, "phase=sandbox_prepare elapsed_ms={}", watch);
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(error = %error, call_id = %call.context.call_id, "a sandboxed call could not be prepared");
                if matches!(error, DaemonError::SandboxUnavailable { .. }) {
                    sandbox.reprobe(Arc::clone(&settings));
                }
                let reason = efr_stdx::with_causes(&error);
                let summary = SandboxSummary {
                    setup_error: Some(reason.clone()),
                    ..SandboxSummary::default()
                };
                return ToolOutcome::error(format!(
                    "[the sandbox could not start: {reason}. The command did not run.]"
                ))
                .with_sandbox(Some(summary));
            }
        };
        let turn_id = call.context.turn_id;
        // NOTE: under the plan lock, so no other call of these projects writes while
        // the snapshot reads them.
        let snapshot = self.snapshot_before(&call.context, &prepared.projects).await;
        if let Some(stop) = self.check_covered(&call.context, &prepared.projects).await {
            if let Some((taken, _)) = snapshot {
                taken.discard();
            }
            remove_call_dir(prepared.run.dir.clone()).await;
            return ToolOutcome::error(stop);
        }
        let watch = Stopwatch::start();
        sandbox
            .turns()
            .before_call(
                turn_id,
                self.clock.now(),
                &prepared.projects,
                &settings.sandbox.surface_files,
                sandbox.git(),
                &self.home,
            )
            .await;
        tracing::debug!(phase = "surface_before", elapsed_ms = %watch, "phase=surface_before elapsed_ms={}", watch);
        let context = context.with_sandbox(Some(prepared.run.clone()));
        let started = prepared.started.clone();
        let notes = prepared.notes.clone();
        let mut sink = CallSink {
            out,
            connections: &self.connections,
            conversation_id: call.context.conversation_id,
        };
        let invoke = self.registry.invoke(&call.name, context, call.input, &mut sink);
        let watch = Stopwatch::start();
        let result = lock::run_holding(prepared.guard, &started, &*self.clock, invoke).await;
        tracing::debug!(phase = "sandboxed_run", elapsed_ms = %watch, "phase=sandboxed_run elapsed_ms={}", watch);
        // NOTE: a launcher that wrote no result still runs (a call that answered at its
        // timeout) or was lost, so its command can write after the snapshot below; the
        // turn's record of its roots is then not complete (plan PB39).
        let ended =
            matches!(&result, Ok(result) if result.sandbox.is_some() || result.sandbox_failed);
        let mut outcome = match result {
            Ok(result) => {
                if result.sandbox_failed {
                    tracing::info!(call_id = %call.context.call_id, "a sandboxed call could not start; checking the sandbox again");
                    sandbox.reprobe(Arc::clone(&settings));
                }
                if let Some(summary) = &result.sandbox {
                    sandbox.turns().changes(turn_id, &summary.surface_changes);
                }
                // NOTE: a process that the launcher could not end (one that sudo left
                // running as root), or a call whose launcher was lost after the start,
                // may still hold the shell's terminal, and the next line typed there
                // could be a password. The shell goes: zsh hangs up its jobs, stopped
                // ones too, and the next call starts a new shell on a new terminal.
                if result.shell_tainted {
                    let conversation = call.context.conversation_id;
                    tracing::warn!(%conversation, "a sandboxed call may have left a process on the hidden shell's terminal; closing the shell");
                    if let Err(error) = self.shells.close(conversation).await {
                        tracing::warn!(%conversation, error = %error, "could not close the hidden shell");
                    }
                }
                // NOTE: a call that ended has nothing left in its dir that anyone reads;
                // one that runs on past its timeout still needs it.
                if result.sandbox.is_some() || result.sandbox_failed {
                    let watch = Stopwatch::start();
                    remove_call_dir(prepared.run.dir.clone()).await;
                    tracing::debug!(phase = "call_dir_remove", elapsed_ms = %watch, "phase=call_dir_remove elapsed_ms={}", watch);
                }
                outcome(result)
            }
            Err(error) => ToolOutcome::error(for_model(&error)),
        };
        let snapshot = snapshot.map(|(taken, limits)| (taken.runs_on(!ended), limits));
        outcome.changes = self.snapshot_after(snapshot).await;
        if !notes.is_empty() {
            outcome.output = format!("{}\n{}", outcome.output, notes.join("\n"));
        }
        outcome
    }
}

impl DaemonToolbox {
    /// Runs a call of the registry: through the sandbox's launcher, or here.
    async fn run_call(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome {
        // NOTE: a call that must run through the sandbox's launcher never falls back to
        // the hidden shell.
        if call.context.launch.uses_launcher() {
            return self.invoke_sandboxed(call, out).await;
        }
        // NOTE: a file write takes the plan lock of its project for its own work, so it
        // never races the plan of a call that is about to start there.
        let _writing = self.lock_write(&call).await;
        let shell = call.name == ShellTool::NAME;
        let snapshot = if shell { self.snapshot_before(&call.context, &[]).await } else { None };
        if FILE_WRITERS.contains(&call.name.as_str()) {
            self.snapshot_before_write(&call).await;
            let projects = match &self.sandbox {
                Some((sandbox, _)) => sandbox.projects().await,
                None => Vec::new(),
            };
            if let Some(stop) = self.check_covered(&call.context, &projects).await {
                return ToolOutcome::error(stop);
            }
        }
        let context = self.context(&call.context);
        let call_context = call.context.clone();
        let mut sink = CallSink {
            out,
            connections: &self.connections,
            conversation_id: call.context.conversation_id,
        };
        let mut outcome =
            match self.registry.invoke(&call.name, context, call.input, &mut sink).await {
                Ok(result) => {
                    let written = result.written.clone();
                    let mut outcome = outcome(result);
                    if let (false, Some(snapshots)) = (written.is_empty(), &self.snapshots) {
                        let (changes, diff) = snapshots.written(&call_context, &written);
                        outcome = outcome.with_changes(changes).with_diff(diff);
                        // NOTE: what a file tool wrote is the turn's own, so undo takes
                        // it back (plan PB38).
                        let mut paths: Vec<PathBuf> = Vec::new();
                        for file in &written {
                            paths.push(file.path.clone());
                            if let WrittenKind::Moved { from } = &file.kind {
                                paths.push(from.clone());
                            }
                        }
                        snapshots.after_write(&call_context, paths).await;
                    }
                    outcome
                }
                Err(error) => ToolOutcome::error(for_model(&error)),
            };
        if shell {
            // NOTE: a command still running in the hidden shell past its timeout can
            // write after the snapshot below.
            let busy = match snapshot {
                Some(_) => self
                    .shells
                    .state(call_context.conversation_id)
                    .await
                    .is_ok_and(|state| state.is_busy()),
                None => false,
            };
            let snapshot = snapshot.map(|(taken, limits)| (taken.runs_on(busy), limits));
            outcome.changes = self.snapshot_after(snapshot).await;
        }
        outcome
    }

    /// Copies the targets of the call's approved exits
    /// ([`ExitNeed::copies`](efr_permissions::ExitNeed::copies)) into the home store
    /// before the call; `None` when there is nothing to copy. `Err` is the text that the
    /// model reads: the call does not run.
    async fn copy_before(&self, call: &CallContext) -> Result<Option<Copying>, String> {
        let mut targets = home::targets(&call.exits);
        let units: Vec<String> = call
            .exits
            .iter()
            .flat_map(|need| &need.copies)
            .filter_map(|ask| match ask {
                CoverAsk::UnitLinks(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        // NOTE: a named change of a unit runs only with a copy of the unit config dir,
        // as a floor file edit does (plan PB14, PB34).
        let unit_dir = units::config_dir(self.home.path());
        for name in &units {
            let (unit, home) = (name.clone(), self.home.path().to_path_buf());
            let found = tokio::task::spawn_blocking(move || units::targets(&unit, &home))
                .await
                .ok()
                .flatten();
            let Some(found) = found else {
                return Err(format!(
                    "efr could not tell which links a change of the unit {name} writes, so the \
                     call did not run"
                ));
            };
            for target in found {
                if !targets.iter().any(|known| known.ask == target.ask) {
                    targets.push(target);
                }
            }
        }
        let mut states: Vec<StateCopy> = Vec::new();
        for copy in call.exits.iter().flat_map(|need| &need.copies).filter_map(StateCopy::of) {
            if !states.contains(&copy) {
                states.push(copy);
            }
        }
        if targets.is_empty() && states.is_empty() {
            return Ok(None);
        }
        let floor = targets.iter().any(|target| target.floor) || !states.is_empty();
        let snapshots = match &self.snapshots {
            Some(snapshots) if snapshots.limits().is_some() => snapshots,
            // NOTE: without the store a floor edit runs only with a copy; another write
            // runs, and its exit record says that no copy covers it.
            _ if floor => {
                return Err("efr keeps no snapshot store here, so it did not run a call \
                            that changes a file that runs code later"
                    .to_owned());
            }
            _ => return Ok(None),
        };
        let target_paths: Vec<PathBuf> = targets
            .iter()
            .filter_map(|target| match &target.ask {
                CoverAsk::Home(path) | CoverAsk::HomeDir(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        let masks = self.copy_masks(&target_paths).await;
        // NOTE: the home store never copies a masked path. For one strictly inside the
        // turn's project or `$SCRATCH`, such as a project `.env`, the snapshot of that
        // root holds it (plan PB12), so a home copy would only say "efr undo cannot
        // revert it" while undo does.
        let roots = snapshots.base_roots(call);
        targets.retain(|target| !home::root_holds(&target.ask, &roots, &masks));
        if targets.is_empty() && states.is_empty() {
            return Ok(None);
        }
        let limits = HomeLimits::of(&self.settings.borrow());
        let (for_prepare, masks_for) = (targets.clone(), masks.clone());
        let mut prepared =
            tokio::task::spawn_blocking(move || home::prepare(&for_prepare, &masks_for, limits))
                .await
                .unwrap_or_else(|_| {
                    Err("copying the files before the change panicked".to_owned())
                })?;
        if !units.is_empty() && !prepared.covers(&CoverAsk::HomeDir(unit_dir.clone())) {
            return Err(format!(
                "efr could not copy {} before the change, so the call did not run",
                unit_dir.display()
            ));
        }
        // NOTE: a named change of the crontab or of linger runs only with a copy of the
        // state before it, as a floor file edit does (plan PB14, PB20).
        for copy in states {
            let (entry, bytes) = state::read(self.state_runner(), copy).await;
            if matches!(entry.state, HomeState::NotCovered(_)) {
                return Err(format!(
                    "efr could not read {} before the change, so the call did not run",
                    entry.path.display()
                ));
            }
            prepared.push_state(entry, bytes);
        }
        let promised: Vec<CoverAsk> = {
            let covered = self.covered.lock().unwrap_or_else(PoisonError::into_inner);
            covered
                .get(&call.call_id)
                .map(|(_, answers)| {
                    answers
                        .iter()
                        .filter(|(_, coverage)| coverage.is_covered())
                        .map(|(ask, _)| ask.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        if targets
            .iter()
            .any(|target| promised.contains(&target.ask) && !prepared.covers(&target.ask))
        {
            return Err(HOME_COPY_CHANGED.to_owned());
        }
        let watch = Stopwatch::start();
        let reader = home::Reader::new(&prepared);
        let copy = snapshots
            .store()
            .home_before(call.conversation_id, call.turn_id, reader)
            .await
            .map_err(|error| {
                format!(
                    "efr could not copy the files of this call into its snapshot store before \
                     the change, so the call did not run: {}",
                    efr_stdx::with_causes(&error)
                )
            })?;
        let record = SnapshotRecord {
            root: "home".to_owned(),
            phase: SnapshotPhase::Pre,
            commit: copy.before.as_str().to_owned(),
            files: u32::try_from(prepared.entries.len()).unwrap_or(u32::MAX),
            skipped: u32::try_from(prepared.not_covered.len()).unwrap_or(u32::MAX),
            ms: u64::try_from(watch.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        let mut names: Vec<String> = targets
            .iter()
            .filter_map(|target| match &target.ask {
                CoverAsk::Home(path) | CoverAsk::HomeDir(path) => {
                    Some(self.shown_home(target.link.as_deref().unwrap_or(path)))
                }
                _ => None,
            })
            .collect();
        names.extend(
            prepared
                .entries
                .iter()
                .filter(|entry| StateCopy::from_path(&entry.path).is_some())
                .map(|entry| entry.path.display().to_string()),
        );
        self.tally(call.turn_id, |tally| {
            tally.records.push(record);
            tally.pending.push((call.call_id, names));
        });
        Ok(Some(Copying { copy, before: prepared, masks, limits }))
    }

    /// `path` as the end of a turn names a home file: below `~/`, else absolute.
    fn shown_home(&self, path: &Path) -> String {
        match path.strip_prefix(self.home.path()) {
            Ok(below) => format!("~/{}", below.display()),
            Err(_) => path.display().to_string(),
        }
    }

    /// The copy after the call of `copying`, and each changed home file. A copy that
    /// fails leaves its call unfinished in the store, so undo leaves its paths alone.
    async fn copy_after(&self, call: &CallContext, copying: Copying) -> Vec<FileBackup> {
        let Some(snapshots) = &self.snapshots else { return Vec::new() };
        let Copying { copy, before, masks, limits } = copying;
        let prepared = before.clone();
        let mut after = tokio::task::spawn_blocking(move || home::after(&prepared, &masks, limits))
            .await
            .unwrap_or_default();
        for copy in before.entries.iter().filter_map(|entry| StateCopy::from_path(&entry.path)) {
            let (entry, bytes) = state::read(self.state_runner(), copy).await;
            after.push_state(entry, bytes);
        }
        let deltas = match snapshots.store().home_after(&copy, home::Reader::new(&after)).await {
            Ok(deltas) => deltas,
            Err(error) => {
                // NOTE: the copy stays pending, so the end of the turn names its targets.
                tracing::warn!(error = %efr_stdx::with_causes(&error), call_id = %call.call_id, "the copy of home files after a call failed");
                return Vec::new();
            }
        };
        let changed = home::changed(&deltas, &before, &after);
        let shown = |path: &Path| self.shown_home(path);
        self.tally(call.turn_id, |tally| {
            tally.pending.retain(|(pending, _)| *pending != call.call_id);
            for backup in &changed {
                match backup.reason {
                    None => {
                        if !tally.changed.contains(&backup.path) {
                            tally.changed.push(backup.path.clone());
                        }
                    }
                    Some(reason) => {
                        let line = format!("{} ({})", shown(&backup.path), reason_text(reason));
                        if !tally.not_covered.contains(&line) {
                            tally.not_covered.push(line);
                        }
                    }
                }
            }
        });
        changed
    }

    /// The programs of the state copies of cron and linger, with this toolbox's clock.
    fn state_runner(&self) -> state::Runner<'_> {
        state::Runner { programs: &self.programs, clock: &*self.clock }
    }

    /// Notes that the turn `turn` ran in `auto`; the oldest turns go past
    /// [`MAX_AUTO_TURNS`].
    fn mark_auto(&self, turn: TurnId) {
        let mut auto = self.auto_turns.lock().unwrap_or_else(PoisonError::into_inner);
        auto.insert(turn);
        if auto.len() > MAX_AUTO_TURNS
            && let Some(oldest) = auto.iter().min().copied()
        {
            auto.remove(&oldest);
        }
    }

    /// Drops what the facts of the call `call` said, once it ran or stopped.
    fn forget_call(&self, call: CallId) {
        self.covered.lock().unwrap_or_else(PoisonError::into_inner).remove(&call);
    }

    /// Changes the home tally of the turn `turn`.
    fn tally(&self, turn: TurnId, change: impl FnOnce(&mut HomeTally)) {
        let mut turns = self.home_turns.lock().unwrap_or_else(PoisonError::into_inner);
        change(turns.entry(turn).or_default());
    }

    /// Copies what `paths` hold into the home store before a cache delete, within the
    /// limits; over them the delete runs with no copy, and the end of the turn says so.
    async fn copy_cache(&self, call: &CallContext, paths: &[PathBuf]) -> Option<Copying> {
        let snapshots = self.snapshots.as_ref()?;
        snapshots.limits()?;
        let targets: Vec<home::Target> = paths
            .iter()
            .map(|path| home::Target {
                ask: CoverAsk::HomeDir(path.clone()),
                floor: false,
                link: None,
            })
            .collect();
        let masks = self.copy_masks(paths).await;
        let limits = HomeLimits::of(&self.settings.borrow());
        let for_prepare = masks.clone();
        let prepared =
            tokio::task::spawn_blocking(move || home::prepare(&targets, &for_prepare, limits))
                .await
                .ok()?;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(reason) => {
                tracing::info!(call_id = %call.call_id, reason, "a cache delete runs with no copy");
                return None;
            }
        };
        let copy = match snapshots
            .store()
            .home_before(call.conversation_id, call.turn_id, home::Reader::new(&prepared))
            .await
        {
            Ok(copy) => copy,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "a cache delete runs with no copy");
                return None;
            }
        };
        let names: Vec<String> = paths.iter().map(|path| self.shown_home(path)).collect();
        self.tally(call.turn_id, |tally| tally.pending.push((call.call_id, names)));
        Some(Copying { copy, before: prepared, masks, limits })
    }

    /// The masks of a home copy of `paths`, or of an undo that writes them:
    /// [`masks`](Self::masks), and the project `.env` files that the plan of a call
    /// masks in each registered project that holds one of `paths` or lies below one
    /// (plan PB14).
    async fn copy_masks(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut masks = self.masks();
        let Some((sandbox, _)) = &self.sandbox else { return masks };
        let projects = sandbox.projects().await;
        let globs = self.settings.borrow().sandbox.mask_globs.clone();
        let paths = paths.to_vec();
        let found = tokio::task::spawn_blocking(move || env_masks(&projects, &paths, &globs)).await;
        masks.extend(found.unwrap_or_default());
        masks
    }

    /// The secrets and masks that efrd never deletes or copies: the engine's secrets,
    /// the sandbox-only masks and `sandbox.mask`, from the latest engine and settings.
    fn masks(&self) -> Vec<PathBuf> {
        let home = self.home.path();
        let secrets = match &self.sandbox {
            Some((_, engine)) => engine.borrow().locations().secret_paths(),
            None => efr_permissions::Locations::new(home)
                .map(|locations| locations.secret_paths())
                .unwrap_or_default(),
        };
        let settings = Arc::clone(&self.settings.borrow());
        let mut masks = cache_deletes::masks(home, &settings, secrets);
        // NOTE: the copy's refused list equals the plan's masks (plan PB14).
        if let Some((sandbox, _)) = &self.sandbox {
            masks.extend(sandbox.host_masks());
        }
        masks
    }
}

/// The registered projects `all` that a shell call may write as `write_projects` says,
/// besides the turn's own: none with `turn`, else each one below the home directory.
/// Its covered target names one, so the plan of the call writes it, and the check under
/// the lock finds it again (plan PB15).
fn cover_projects(all: Vec<PathBuf>, write_projects: WriteProjects, home: &Path) -> Vec<PathBuf> {
    if write_projects == WriteProjects::Turn {
        return Vec::new();
    }
    all.into_iter().filter(|project| !efr_sandbox::too_wide(project, home)).collect()
}

/// The project `.env` files of `globs` in each project of `projects` that holds one of
/// `paths` or lies below one, as the plan of a call masks them (plan PB14). It blocks.
fn env_masks(projects: &[PathBuf], paths: &[PathBuf], globs: &[String]) -> Vec<PathBuf> {
    projects
        .iter()
        .filter(|project| {
            paths.iter().any(|path| path.starts_with(project) || project.starts_with(path))
        })
        .flat_map(|project| crate::sandbox::plan::env_files(project, globs))
        .collect()
}

/// True for an ask that the home store answers, not a root's snapshot: a home file or
/// dir, the links of a unit, the crontab or linger.
fn home_side(ask: &CoverAsk) -> bool {
    matches!(
        ask,
        CoverAsk::Home(_)
            | CoverAsk::HomeDir(_)
            | CoverAsk::UnitLinks(_)
            | CoverAsk::CronTable
            | CoverAsk::Linger(_)
    )
}

/// What the model reads for a call of the edit tool `name` that its request did not
/// offer: the call did not run, and which tool changes files.
fn not_offered(name: &str, edit: efr_provider::EditTool) -> String {
    format!(
        "This model has no {name} tool, and the call did not run. Use the {} tool to \
         change a file.",
        edit_tool_name(edit)
    )
}

/// The provider's definition of a registered tool: a field-by-field copy, with a
/// freeform tool's grammar.
pub(crate) fn definition(spec: ToolSpec) -> ToolDefinition {
    let mut definition = ToolDefinition::function(spec.name, spec.description, spec.input_schema);
    definition.grammar = spec.grammar.map(|grammar| match grammar {
        efr_tools::ToolGrammar::Lark(definition) => ProviderGrammar::lark(definition),
    });
    definition
}

#[async_trait]
impl Toolbox for DaemonToolbox {
    fn definitions(&self, edit: efr_provider::EditTool) -> Vec<ToolDefinition> {
        let mut definitions: Vec<ToolDefinition> = self
            .registry
            .specs()
            .into_iter()
            .filter(|spec| offered(&spec.name, edit))
            .map(definition)
            .collect();
        definitions.push(SettingsTool::definition());
        definitions
    }

    async fn requirements(&self, call: &ToolCall) -> Result<Requirements, String> {
        if call.name == settings_tool::NAME {
            return self.settings_tool.requirements(call).await;
        }
        // NOTE: the registry keeps both edit tools, so a model could name the one that
        // its request did not offer, such as `apply_patch` from an earlier model's calls.
        if !offered(&call.name, call.context.edit_tool) {
            return Err(not_offered(&call.name, call.context.edit_tool));
        }
        let context = self.context(&call.context);
        let declared = self
            .registry
            .requirements(&call.name, &context, &call.input)
            .map_err(|error| for_model(&error))?;
        // NOTE: a tool declares paths as written. What one reaches through a symbolic
        // link is read from the disk here, on the blocking pool, so the engine judges
        // `cat notes`, where `notes` links into `~/.ssh`, as a read of the key too.
        let home = self.home.clone();
        let watch = Stopwatch::start();
        let resolved = tokio::task::spawn_blocking(move || declared.with_real_paths(&home)).await;
        tracing::debug!(phase = "real_paths", elapsed_ms = %watch, "phase=real_paths elapsed_ms={}", watch);
        match resolved {
            Ok(declared) => {
                // NOTE: only `auto` reads the facts, and collecting them runs git in
                // directories that the model can write, so other modes skip it.
                let watch = Stopwatch::start();
                let facts =
                    if call.context.auto { self.facts(call, &declared).await } else { None };
                if facts.is_some() {
                    tracing::debug!(phase = "facts", elapsed_ms = %watch, "phase=facts elapsed_ms={}", watch);
                }
                let requirements = permission_requirements(declared);
                Ok(match facts {
                    Some(facts) => requirements.with_facts(facts),
                    None => requirements,
                })
            }
            Err(error) => {
                tracing::warn!(error = %error, "the paths of a tool call could not be resolved");
                Err("efr could not check where the paths of this call lead on disk, so it did \
                     not run; try again"
                    .to_owned())
            }
        }
    }

    async fn preview(&self, call: &ToolCall) -> Option<String> {
        if call.name == settings_tool::NAME {
            return self.settings_tool.preview(call).await;
        }
        let context = self.context(&call.context);
        let preview = self.registry.preview(&call.name, &context, &call.input).await?;
        // NOTE: the edit tools name absolute paths; the question shows them as the
        // finished call's diff does.
        match (&self.snapshots, EDIT_TOOLS.contains(&call.name.as_str())) {
            (Some(snapshots), true) => Some(snapshots.shown_preview(&call.context, &preview)),
            _ => Some(preview),
        }
    }

    fn takes_manual_input(&self, name: &str, input: &Value) -> bool {
        self.registry.takes_manual_input(name, input)
    }

    async fn invoke(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome {
        if call.name == settings_tool::NAME {
            return self.settings_tool.invoke(call).await;
        }
        // NOTE: the one place that copies home files before an approved write (plan
        // PD9, PB13), so `write_file`, `edit`, `apply_patch` and a line in the exit
        // child share it. A copy that fails stops the call.
        let context = call.context.clone();
        if context.auto {
            self.mark_auto(context.turn_id);
        }
        let copying = match self.copy_before(&context).await {
            Ok(copying) => copying,
            Err(reason) => {
                self.forget_call(context.call_id);
                return ToolOutcome::error(reason);
            }
        };
        let outcome = self.run_call(call, out).await;
        self.forget_call(context.call_id);
        let Some(copying) = copying else { return outcome };
        let backups = self.copy_after(&context, copying).await;
        outcome.with_backups(backups)
    }

    async fn shell_cwd(&self, conversation_id: ConversationId) -> Option<PathBuf> {
        // NOTE: no shell is the common answer, and the turn then reads the log.
        self.shells
            .state(conversation_id)
            .await
            .ok()
            .map(|state| state.effective_cwd().to_path_buf())
    }

    async fn jobs(&self, conversation_id: ConversationId) -> Option<Vec<String>> {
        // NOTE: no shell runs, so none of its jobs either.
        let Ok(state) = self.shells.state(conversation_id).await else {
            return Some(Vec::new());
        };
        let pid = state.pid;
        tokio::task::spawn_blocking(move || jobs::of(Path::new("/proc"), pid)).await.ok().flatten()
    }

    async fn move_shell(&self, conversation_id: ConversationId, dir: &Path) {
        if let Err(error) = self.shells.move_to(conversation_id, dir).await {
            tracing::info!(%conversation_id, %error, "the hidden shell stays where it is");
        }
    }

    async fn restore_quarantine(
        &self,
        call: &CallContext,
        changes: &[SurfaceChange],
    ) -> Result<(), String> {
        let Some((sandbox, _)) = &self.sandbox else {
            return Err("this efrd keeps no quarantine".to_owned());
        };
        let dir = sandbox.quarantine_dir(call.conversation_id, call.call_id);
        let changes = changes.to_vec();
        tokio::task::spawn_blocking(move || quarantine::restore(&dir, &changes))
            .await
            .unwrap_or_else(|_| Err("moving the changes back panicked".to_owned()))
    }

    async fn delete_cache_paths(
        &self,
        call: &CallContext,
        paths: &[PathBuf],
    ) -> Result<CacheDeleteOutcome, String> {
        if self.sandbox.is_none() {
            return Err("this efrd runs no sandbox, so the deletes stayed private".to_owned());
        }
        let settings = Arc::clone(&self.settings.borrow());
        let caches = cache_deletes::caches(self.home.path(), &settings);
        let masks = self.masks();
        let checked = match cache_deletes::check(paths, &caches, &masks) {
            Ok(checked) => checked,
            Err(path) => {
                tracing::warn!(call_id = %call.call_id, path = %path.display(), "a cache delete is not in a tool cache or reaches a pin or a mask; nothing is deleted");
                return Err(cache_deletes::REFUSED.to_owned());
            }
        };
        let copying = self.copy_cache(call, paths).await;
        let applied = tokio::task::spawn_blocking(move || cache_deletes::apply(&checked)).await;
        let applied =
            applied.unwrap_or_else(|_| Err("deleting the cache paths panicked".to_owned()));
        if let Some(copying) = copying {
            self.copy_after(call, copying).await;
        } else {
            let over = reason_text(efr_protocol::UndoReason::OverLimit);
            let shown: Vec<String> =
                paths.iter().map(|path| format!("{} ({over})", self.shown_home(path))).collect();
            if self.snapshots.is_some() {
                self.tally(call.turn_id, |tally| tally.not_covered.extend(shown));
            }
        }
        applied
    }

    async fn turn_report(
        &self,
        _conversation_id: ConversationId,
        turn_id: TurnId,
    ) -> Vec<ReportedFile> {
        let Some((sandbox, _)) = &self.sandbox else { return Vec::new() };
        let patterns = self.settings.borrow().sandbox.surface_files.clone();
        sandbox.turns().finish(turn_id, &patterns, sandbox.git(), sandbox.home()).await
    }

    async fn turn_changes(&self, conversation_id: ConversationId, turn_id: TurnId) -> TurnChanges {
        self.covered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, (turn, _)| *turn != turn_id);
        let tally = self.home_turns.lock().unwrap_or_else(PoisonError::into_inner).remove(&turn_id);
        let mut tally = tally.unwrap_or_default();
        let unfinished = reason_text(efr_protocol::UndoReason::CallUnfinished);
        for name in std::mem::take(&mut tally.pending).into_iter().flat_map(|(_, names)| names) {
            let line = format!("{name} ({unfinished})");
            if !tally.not_covered.contains(&line) {
                tally.not_covered.push(line);
            }
        }
        let Some(snapshots) = &self.snapshots else { return TurnChanges::default() };
        let mut ended = snapshots.turn_end(turn_id).await;
        if ended.auto {
            self.mark_auto(turn_id);
        }
        // NOTE: a turn of `auto` whose calls wrote only home files snapshotted no root.
        ended.auto |=
            self.auto_turns.lock().unwrap_or_else(PoisonError::into_inner).contains(&turn_id);
        let enabled = self.settings.borrow().undo.enabled;
        let home_files = u32::try_from(tally.changed.len()).unwrap_or(u32::MAX);
        let runs = self.call_runs(conversation_id);
        let undo = snapshot::undo_info(&ended, home_files, &tally.not_covered, enabled, runs);
        let mut records = snapshot::records(&ended);
        records.extend(tally.records);
        TurnChanges { changes: ended.end.changes, undo, snapshots: records }
    }

    async fn undo(&self, request: UndoRequest) -> Result<TurnUndoResult, UndoRefused> {
        self.undo_turn(request).await
    }

    fn call_runs(&self, conversation_id: ConversationId) -> bool {
        // NOTE: a call dir with `started` and no `result.json`: its launcher still runs,
        // also when its tool call answered at the timeout (plan PB39).
        self.sandbox.as_ref().is_some_and(|(sandbox, _)| sandbox.launcher_runs(conversation_id))
    }

    async fn cancel(&self, call: &CallContext) {
        // NOTE: only the shell tool leaves work behind, and Ctrl+C at an idle prompt
        // does nothing, so every interrupted call interrupts the conversation's shell.
        if let Err(error) = self.shells.interrupt(call.conversation_id).await {
            tracing::debug!(error = %error, call_id = %call.call_id, "nothing to interrupt in the hidden shell");
        }
    }
}

/// Why no copy covers a home file, in a few words for the end of a turn.
fn reason_text(reason: efr_protocol::UndoReason) -> &'static str {
    use efr_protocol::UndoReason;
    match reason {
        UndoReason::Masked => "masked",
        UndoReason::OverLimit => "over the copy limits",
        UndoReason::CallUnfinished => "the call did not finish",
        _ => "no copy",
    }
}

/// Removes the dir of a sandboxed call that ended. A dir that cannot go stays in efr's
/// runtime root, which the next boot empties.
async fn remove_call_dir(dir: PathBuf) {
    let removed = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&dir)).await;
    if let Ok(Err(error)) = removed {
        tracing::debug!(error = %error, "a sandboxed call's dir stays");
    }
}

/// A call's sink as the tools see it: output and input waits go to the conversation,
/// and whether a person can answer hidden input comes from the live subscriptions.
pub(crate) struct CallSink<'a> {
    pub(crate) out: &'a mut dyn OutputSink,
    pub(crate) connections: &'a Connections,
    pub(crate) conversation_id: ConversationId,
}

impl ToolOutputSink for CallSink<'_> {
    fn update(&mut self, tail: &str, bytes: u64) {
        self.out.update(tail, bytes);
    }

    fn input_changed(&mut self, wait: InputWait, looks_secret: bool) {
        self.out.input_changed(wait, looks_secret);
    }

    fn can_answer_hidden(&mut self) -> bool {
        // NOTE: asked at every look while a command waits for hidden input, so a client
        // that goes away mid-wait stops the command at the next look.
        self.connections.answerers(self.conversation_id) > 0
    }

    fn can_answer(&mut self) -> bool {
        // NOTE: asked once per quiet period past the timeout of an approved interactive
        // call, so the call answers the model soon after the last client goes away.
        self.connections.answerers(self.conversation_id) > 0
    }
}

/// `ToolRequirements` as the permission engine reads them.
pub(crate) fn permission_requirements(declared: ToolRequirements) -> Requirements {
    let mut requirements = Requirements::none();
    for access in declared.paths {
        requirements = match access.mode {
            AccessMode::Read => requirements.with_read(access.path),
            AccessMode::ReadTree => requirements.with_read_tree(access.path),
            // NOTE: a mode this build does not know may change the file, so it counts as
            // a write, which the engine judges more strictly.
            _ => requirements.with_write(access.path),
        };
    }
    if let Some(command) = declared.command {
        requirements = requirements.with_command(command);
    }
    if let Some(dir) = declared.command_dir {
        requirements = requirements.with_command_dir(dir);
    }
    if declared.network {
        requirements = requirements.with_network();
    }
    if declared.interactive {
        requirements = requirements.with_interactive();
    }
    if let Some(needs) = declared.needs {
        requirements = requirements.with_needs(needs);
    }
    if declared.nested {
        requirements = requirements.with_nested();
    }
    if declared.destructive {
        requirements = requirements.with_destructive();
    }
    requirements
}

/// A tool result as the conversation records it.
pub(crate) fn outcome(result: ToolResult) -> ToolOutcome {
    let base = if result.is_error {
        ToolOutcome::error(result.output)
    } else {
        ToolOutcome::ok(result.output)
    };
    base.with_truncated(result.truncated)
        .with_exit_code(result.exit_code)
        .with_sandbox(result.sandbox)
}

/// A tool error with its causes, for the model: the reason a file could not be read
/// is what lets it choose another way.
pub(crate) fn for_model(error: &ToolError) -> String {
    efr_stdx::with_causes(error)
}

/// The write journal until the store keeps one: each original is logged by path and
/// size and then dropped, so `undo` has nothing to restore yet.
#[derive(Debug, Clone, Copy)]
struct LogJournal;

#[async_trait]
impl WriteJournal for LogJournal {
    async fn record(&self, entry: JournalEntry) -> Result<(), ToolError> {
        tracing::debug!(call_id = %entry.ids.call_id, path = %entry.snapshot.path.display(), "a file is about to be written");
        Ok(())
    }
}

#[cfg(test)]
mod tests;
