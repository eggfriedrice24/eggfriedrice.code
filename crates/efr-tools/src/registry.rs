//! The registry: the tools a conversation offers, by name.

use std::sync::Arc;

use serde_json::Value;

use crate::{Tool, ToolContext, ToolError, ToolOutputSink, ToolRequirements, ToolResult, ToolSpec};

/// The tools the model may call.
///
/// It gives out the specs for the provider request and dispatches
/// [`requirements`](Self::requirements) and [`invoke`](Self::invoke) by name, and it
/// never hands out a tool: the conversation reaches the hidden shell only through a
/// `shell` call that went past the permission check, never through a handle it could
/// call directly.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    spec: ToolSpec,
    tool: Arc<dyn Tool>,
}

impl ToolRegistry {
    /// A registry with no tools.
    pub fn new() -> Self {
        ToolRegistry::default()
    }

    /// Adds `tool` under the name in its spec. Fails when the name is taken or is not
    /// 1 to 64 ASCII letters, digits, `_`, `-` or `.`.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let spec = tool.spec();
        if !valid_name(&spec.name) {
            return Err(ToolError::InvalidName { name: spec.name });
        }
        if self.entries.iter().any(|entry| entry.spec.name == spec.name) {
            return Err(ToolError::DuplicateTool { name: spec.name });
        }
        self.entries.push(Entry { spec, tool });
        Ok(())
    }

    /// The specs of every tool, in the order they were registered.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.entries.iter().map(|entry| entry.spec.clone()).collect()
    }

    /// True when a tool is registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.entry(name).is_ok()
    }

    /// What a call of the tool `name` with `input` needs.
    pub fn requirements(
        &self,
        name: &str,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<ToolRequirements, ToolError> {
        self.entry(name)?.tool.requirements(ctx, input)
    }

    /// True when a call of the tool `name` with `input` takes an input that the user
    /// chooses to type while it reports no wait; false for an unknown tool.
    pub fn takes_manual_input(&self, name: &str, input: &Value) -> bool {
        self.entry(name).is_ok_and(|entry| entry.tool.takes_manual_input(input))
    }

    /// What a call of the tool `name` with `input` would change, for its approval;
    /// `None` for an unknown tool or a call without a preview.
    pub async fn preview(&self, name: &str, ctx: &ToolContext, input: &Value) -> Option<String> {
        let tool = Arc::clone(&self.entry(name).ok()?.tool);
        tool.preview(ctx, input).await
    }

    /// Runs a call of the tool `name`. The caller has checked the call's requirements
    /// with the permission engine first.
    pub async fn invoke(
        &self,
        name: &str,
        ctx: ToolContext,
        input: Value,
        out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        let tool = Arc::clone(&self.entry(name)?.tool);
        tool.invoke(ctx, input, out).await
    }

    fn entry(&self, name: &str) -> Result<&Entry, ToolError> {
        self.entries
            .iter()
            .find(|entry| entry.spec.name == name)
            .ok_or_else(|| ToolError::UnknownTool { name: name.to_owned() })
    }
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests;
