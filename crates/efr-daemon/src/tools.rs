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
//! for its timeout.

use std::error::Error;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use efr_config::{Settings, SudoCache};
use efr_conversation::{CallContext, OutputSink, ToolCall, ToolOutcome, Toolbox};
use efr_permissions::Requirements;
use efr_protocol::{ConversationId, InputWait};
use efr_provider::ToolDefinition;
use efr_scope::Home;
use efr_shell::ShellSessions;
use efr_stdx::time::Clock;
use efr_tools::{
    AccessMode, CallIds, JournalEntry, ReadFileTool, ShellTool, ToolContext, ToolError,
    ToolOutputSink, ToolRegistry, ToolRequirements, ToolResult, WriteFileTool, WriteJournal,
};
use tokio::sync::watch;

use crate::DaemonError;
use crate::connections::Connections;

mod settings_tool;

pub(crate) use settings_tool::SettingsTool;

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
        }
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
    }

    /// True when the latest settings make the hidden shell forget sudo's credentials
    /// after each call, read at each call so a change reaches the next one.
    fn forgets_credentials(&self) -> bool {
        self.settings.borrow().shell.sudo_cache == SudoCache::PerCall
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
            Ok(declared) => Ok(permission_requirements(declared)),
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

    async fn invoke(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome {
        if call.name == settings_tool::NAME {
            return self.settings_tool.invoke(call).await;
        }
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
        self.shells.state(conversation_id).await.ok().map(|state| state.cwd)
    }

    async fn cancel(&self, call: &CallContext) {
        // NOTE: only the shell tool leaves work behind, and Ctrl+C at an idle prompt
        // does nothing, so every interrupted call interrupts the conversation's shell.
        if let Err(error) = self.shells.interrupt(call.conversation_id).await {
            tracing::debug!(error = %error, call_id = %call.call_id, "nothing to interrupt in the hidden shell");
        }
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

    fn input_changed(&mut self, wait: InputWait) {
        self.out.input_changed(wait);
    }

    fn can_answer_hidden(&mut self) -> bool {
        // NOTE: asked at every look while a command waits for hidden input, so a client
        // that goes away mid-wait stops the command at the next look.
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
    requirements
}

/// A tool result as the conversation records it.
pub(crate) fn outcome(result: ToolResult) -> ToolOutcome {
    let base = if result.is_error {
        ToolOutcome::error(result.output)
    } else {
        ToolOutcome::ok(result.output)
    };
    base.with_truncated(result.truncated).with_exit_code(result.exit_code)
}

/// A tool error with its causes, for the model: the reason a file could not be read
/// is what lets it choose another way.
pub(crate) fn for_model(error: &ToolError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let _ = write!(text, ": {cause}");
        source = cause.source();
    }
    text
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
