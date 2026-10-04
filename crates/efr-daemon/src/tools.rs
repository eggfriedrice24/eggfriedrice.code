//! The tools: the registry composed from `efr-tools`, offered to conversations through
//! the `Toolbox` trait of `efr-conversation`.
//!
//! `efr-conversation` cannot depend on `efr-tools` (the chain reaches `efr-shell`, a
//! forbidden edge), so [`DaemonToolbox`] copies between the two, field by field: specs
//! to tool definitions, tool requirements to permission requirements, results to
//! outcomes. It never decides a permission; the conversation's check point does.

use std::error::Error;
use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use efr_conversation::{CallContext, OutputSink, ToolCall, ToolOutcome, Toolbox};
use efr_permissions::Requirements;
use efr_provider::ToolDefinition;
use efr_scope::Home;
use efr_shell::ShellSessions;
use efr_stdx::time::Clock;
use efr_tools::{
    AccessMode, CallIds, JournalEntry, ReadFileTool, ShellTool, ToolContext, ToolError,
    ToolRegistry, ToolRequirements, ToolResult, WriteFileTool, WriteJournal,
};

use crate::DaemonError;

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

/// The registry as a conversation's `Toolbox`.
#[derive(Debug)]
pub(crate) struct DaemonToolbox {
    registry: ToolRegistry,
    shells: ShellSessions,
    home: Home,
    clock: Arc<dyn Clock>,
    journal: Arc<dyn WriteJournal>,
}

impl DaemonToolbox {
    pub(crate) fn new(
        registry: ToolRegistry,
        shells: ShellSessions,
        home: Home,
        clock: Arc<dyn Clock>,
    ) -> Self {
        DaemonToolbox { registry, shells, home, clock, journal: Arc::new(LogJournal) }
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
    }
}

#[async_trait]
impl Toolbox for DaemonToolbox {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry
            .specs()
            .into_iter()
            .map(|spec| ToolDefinition {
                name: spec.name,
                description: spec.description,
                input_schema: spec.input_schema,
            })
            .collect()
    }

    fn requirements(&self, call: &ToolCall) -> Result<Requirements, String> {
        let context = self.context(&call.context);
        self.registry
            .requirements(&call.name, &context, &call.input)
            .map(permission_requirements)
            .map_err(|error| for_model(&error))
    }

    async fn invoke(&self, call: ToolCall, out: &mut dyn OutputSink) -> ToolOutcome {
        let context = self.context(&call.context);
        let mut sink = |tail: &str, bytes: u64| out.update(tail, bytes);
        match self.registry.invoke(&call.name, context, call.input, &mut sink).await {
            Ok(result) => outcome(result),
            Err(error) => ToolOutcome::error(for_model(&error)),
        }
    }

    async fn cancel(&self, call: &CallContext) {
        // NOTE: only the shell tool leaves work behind, and Ctrl+C at an idle prompt
        // does nothing, so every interrupted call interrupts the conversation's shell.
        if let Err(error) = self.shells.interrupt(call.conversation_id).await {
            tracing::debug!(error = %error, call_id = %call.call_id, "nothing to interrupt in the hidden shell");
        }
    }
}

/// `ToolRequirements` as the permission engine reads them.
pub(crate) fn permission_requirements(declared: ToolRequirements) -> Requirements {
    let mut requirements = Requirements::none();
    for access in declared.paths {
        requirements = match access.mode {
            AccessMode::Read => requirements.with_read(access.path),
            // NOTE: a mode this build does not know may change the file, so it counts as
            // a write, which the engine judges more strictly.
            _ => requirements.with_write(access.path),
        };
    }
    if let Some(command) = declared.command {
        requirements = requirements.with_command(command);
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
