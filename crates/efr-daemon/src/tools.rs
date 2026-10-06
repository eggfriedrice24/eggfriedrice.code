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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_config::{Settings, SudoCache};
use efr_conversation::{CallContext, OutputSink, ToolCall, ToolOutcome, Toolbox};
use efr_permissions::{Engine, Requirements};
use efr_protocol::{
    ConversationId, InputWait, ReportedFile, SandboxSummary, SurfaceChange, TurnId,
};
use efr_provider::ToolDefinition;
use efr_scope::Home;
use efr_shell::ShellSessions;
use efr_stdx::time::Clock;
use efr_tools::{
    AccessMode, CallIds, JournalEntry, ReadFileTool, ShellTool, ToolContext, ToolError,
    ToolOutputSink, ToolRegistry, ToolRequirements, ToolResult, WriteFileTool, WriteJournal,
};
use serde_json::Value;
use tokio::sync::watch;

use crate::DaemonError;
use crate::connections::Connections;
use crate::sandbox::{PrepareInput, SandboxService, facts, lock, quarantine};

mod settings_tool;

pub(crate) use settings_tool::SettingsTool;

/// What the model reads for a call that must run through the sandbox's launcher while
/// this toolbox has no sandbox; it never runs in the hidden shell instead.
pub(crate) const SANDBOX_NOT_STARTED: &str =
    "[the sandbox could not start: this efrd does not run the sandbox. The command did not run.]";

/// The registry of milestone 1: the shell, `read_file` and `write_file`.
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
    /// The facts about the files and programs of a shell call that the engine reads in
    /// `auto`; `None` for another tool or without a sandbox.
    async fn facts(
        &self,
        call: &ToolCall,
        declared: &ToolRequirements,
    ) -> Option<efr_permissions::CallFacts> {
        let (sandbox, engine) = self.sandbox.as_ref()?;
        let command = declared.command.as_deref()?;
        let engine = Arc::clone(&engine.borrow());
        let command_dir = declared.command_dir.as_deref();
        let request =
            efr_permissions::exits::fact_requests(command, command_dir, engine.locations())
                .with_needs(declared.needs.as_ref(), command_dir, engine.locations());
        let writes: Vec<PathBuf> = declared
            .paths
            .iter()
            .filter(|access| !matches!(access.mode, AccessMode::Read | AccessMode::ReadTree))
            .map(|access| access.path.clone())
            .collect();
        let rebuildable = self.settings.borrow().sandbox.rebuildable.clone();
        let turn_start = sandbox.turns().started(call.context.turn_id, self.clock.now());
        let input = facts::FactInput {
            request: &request,
            writes: &writes,
            command_dir,
            shell_path: &sandbox.host().path,
            rebuildable: &rebuildable,
            turn_start,
        };
        Some(facts::collect(&input, sandbox.git(), &self.home).await)
    }

    /// Takes the plan lock of the projects that a `write_file` call writes, for the
    /// write; `None` for another tool or without a sandbox.
    async fn lock_write(&self, call: &ToolCall) -> Option<lock::PlanGuard> {
        let (sandbox, _) = self.sandbox.as_ref()?;
        if call.name != WriteFileTool::NAME {
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
        // NOTE: the plan lock blocks every write and every plan in the call's projects,
        // also those of other conversations. A call that queues behind a command still
        // running in this shell (a dev server left at its timeout) waits here, before it
        // takes the lock, so it blocks nobody while it waits.
        if let Some(timeout) = timeout
            && self.shells.until_free(call.context.conversation_id, timeout).await.is_err()
        {
            return ToolOutcome::error(format!(
                "The shell did not become free within {}s, so the command was not run: an \
                 earlier command is still running in it.",
                timeout.as_secs()
            ));
        }
        let input = PrepareInput { settings: &settings, engine: &engine, named_paths };
        let prepared = match sandbox.prepare(&call.context, &input).await {
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
        let context = context.with_sandbox(Some(prepared.run.clone()));
        let started = prepared.started.clone();
        let notes = prepared.notes.clone();
        let mut sink = CallSink {
            out,
            connections: &self.connections,
            conversation_id: call.context.conversation_id,
        };
        let invoke = self.registry.invoke(&call.name, context, call.input, &mut sink);
        let result = lock::run_holding(prepared.guard, &started, &*self.clock, invoke).await;
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
                    remove_call_dir(prepared.run.dir.clone()).await;
                }
                outcome(result)
            }
            Err(error) => ToolOutcome::error(for_model(&error)),
        };
        if !notes.is_empty() {
            outcome.output = format!("{}\n{}", outcome.output, notes.join("\n"));
        }
        outcome
    }
}

#[async_trait]
impl Toolbox for DaemonToolbox {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions: Vec<ToolDefinition> = self
            .registry
            .specs()
            .into_iter()
            .map(|spec| ToolDefinition {
                name: spec.name,
                description: spec.description,
                input_schema: spec.input_schema,
            })
            .collect();
        definitions.push(SettingsTool::definition());
        definitions
    }

    async fn requirements(&self, call: &ToolCall) -> Result<Requirements, String> {
        if call.name == settings_tool::NAME {
            return self.settings_tool.requirements(call).await;
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
        match tokio::task::spawn_blocking(move || declared.with_real_paths(&home)).await {
            Ok(declared) => {
                // NOTE: only `auto` reads the facts, and collecting them runs git in
                // directories that the model can write, so other modes skip it.
                let facts =
                    if call.context.auto { self.facts(call, &declared).await } else { None };
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
        self.registry.preview(&call.name, &context, &call.input).await
    }

    fn takes_manual_input(&self, name: &str, input: &Value) -> bool {
        self.registry.takes_manual_input(name, input)
    }

    async fn invoke(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome {
        if call.name == settings_tool::NAME {
            return self.settings_tool.invoke(call).await;
        }
        // NOTE: a call that must run through the sandbox's launcher never falls back to
        // the hidden shell.
        if call.context.launch.uses_launcher() {
            return self.invoke_sandboxed(call, out).await;
        }
        // NOTE: a file write takes the plan lock of its project for its own work, so it
        // never races the plan of a call that is about to start there.
        let _writing = self.lock_write(&call).await;
        let context = self.context(&call.context);
        let mut sink = CallSink {
            out,
            connections: &self.connections,
            conversation_id: call.context.conversation_id,
        };
        match self.registry.invoke(&call.name, context, call.input, &mut sink).await {
            Ok(result) => outcome(result),
            Err(error) => ToolOutcome::error(for_model(&error)),
        }
    }

    async fn shell_cwd(&self, conversation_id: ConversationId) -> Option<PathBuf> {
        // NOTE: no shell is the common answer, and the turn then reads the log.
        self.shells
            .state(conversation_id)
            .await
            .ok()
            .map(|state| state.effective_cwd().to_path_buf())
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

    async fn turn_report(
        &self,
        _conversation_id: ConversationId,
        turn_id: TurnId,
    ) -> Vec<ReportedFile> {
        let Some((sandbox, _)) = &self.sandbox else { return Vec::new() };
        let patterns = self.settings.borrow().sandbox.surface_files.clone();
        sandbox.turns().finish(turn_id, &patterns, sandbox.git(), sandbox.home()).await
    }

    async fn cancel(&self, call: &CallContext) {
        // NOTE: only the shell tool leaves work behind, and Ctrl+C at an idle prompt
        // does nothing, so every interrupted call interrupts the conversation's shell.
        if let Err(error) = self.shells.interrupt(call.conversation_id).await {
            tracing::debug!(error = %error, call_id = %call.call_id, "nothing to interrupt in the hidden shell");
        }
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
