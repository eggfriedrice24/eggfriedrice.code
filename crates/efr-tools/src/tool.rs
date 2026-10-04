//! The tool boundary: what a tool tells the model, what a call needs, what it returns.

use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{ToolContext, ToolError};

/// One tool the model can call.
///
/// The conversation asks [`requirements`](Tool::requirements) first and passes the
/// answer to the permission engine; only an allowed or approved call reaches
/// [`invoke`](Tool::invoke). A tool never asks for or grants a permission itself
/// (`xtask/src/deps.rs` forbids `efr-tools -> efr-permissions`), so declaring
/// everything a call touches is the tool's whole contract: the engine cannot judge
/// what a tool does not declare.
#[async_trait]
pub trait Tool: Send + Sync + fmt::Debug {
    /// The name, description and input schema the model sees. The registry asks once,
    /// when the tool is registered.
    fn spec(&self) -> ToolSpec;

    /// What a call with `input` needs: every path with its access mode, the command
    /// line it runs, and whether it talks to the network or may wait for input. Pure:
    /// it resolves paths lexically against the context and touches no file.
    fn requirements(&self, ctx: &ToolContext, input: &Value)
    -> Result<ToolRequirements, ToolError>;

    /// Runs the call. Output that grows while the call runs goes to `out`.
    async fn invoke(
        &self,
        ctx: ToolContext,
        input: Value,
        out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError>;
}

/// A tool as the model sees it. The conversation copies it into the provider's own
/// tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolSpec {
    /// The name the model calls the tool by, such as `shell`.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// The JSON Schema of the tool's input object.
    pub input_schema: Value,
}

impl ToolSpec {
    /// A spec with an explicit schema.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        ToolSpec { name: name.into(), description: description.into(), input_schema }
    }

    /// A spec whose schema is generated from the input type `T` (schemars), without the
    /// `$schema` and `title` keys that provider APIs do not want.
    pub fn for_input<T: JsonSchema>(
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        let mut schema = schemars::schema_for!(T);
        schema.remove("$schema");
        schema.remove("title");
        ToolSpec::new(name, description, schema.to_value())
    }
}

/// Parses a call's input as `T`, naming the tool when it does not match.
pub(crate) fn parse_input<T: DeserializeOwned>(tool: &str, input: &Value) -> Result<T, ToolError> {
    T::deserialize(input)
        .map_err(|source| ToolError::InvalidInput { tool: tool.to_owned(), source })
}

/// What a call does with a path. Creating, replacing and deleting a file count as
/// writing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AccessMode {
    /// The call reads the path or lists the directory.
    Read,
    /// The call creates, changes or deletes the path.
    Write,
}

/// One path a call touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAccess {
    /// The absolute path, resolved lexically from the call's input.
    pub path: PathBuf,
    /// What the call does with it.
    pub mode: AccessMode,
}

/// What one call needs, for the permission engine.
///
/// `Debug` shows the command line's length, not its text: a command can carry a
/// secret, such as `export TOKEN=...`.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolRequirements {
    /// The paths the call reads or writes.
    pub paths: Vec<PathAccess>,
    /// The command line the call runs in the hidden shell.
    pub command: Option<String>,
    /// True when the call talks to the network.
    pub network: bool,
    /// True when the call may wait for input at the terminal, such as a `sudo`
    /// password.
    pub interactive: bool,
}

impl ToolRequirements {
    /// No requirements.
    pub fn none() -> Self {
        ToolRequirements::default()
    }

    /// Adds a path the call reads.
    #[must_use]
    pub fn with_read(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(PathAccess { path: path.into(), mode: AccessMode::Read });
        self
    }

    /// Adds a path the call writes.
    #[must_use]
    pub fn with_write(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(PathAccess { path: path.into(), mode: AccessMode::Write });
        self
    }

    /// Sets the command line the call runs.
    #[must_use]
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// Sets whether the call talks to the network.
    #[must_use]
    pub fn with_network(mut self, network: bool) -> Self {
        self.network = network;
        self
    }

    /// Sets whether the call may wait for input at the terminal.
    #[must_use]
    pub fn with_interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }
}

impl fmt::Debug for ToolRequirements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolRequirements")
            .field("paths", &self.paths)
            .field("command", &self.command.as_ref().map(|command| CommandLength(command.len())))
            .field("network", &self.network)
            .field("interactive", &self.interactive)
            .finish()
    }
}

struct CommandLength(usize);

impl fmt::Debug for CommandLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0)
    }
}

/// What a call returns to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolResult {
    /// The text the model sees, truncated in the middle when it is long.
    pub output: String,
    /// True when `output` was truncated.
    pub truncated: bool,
    /// True when the call failed: an error result for the model, or a command that
    /// did not exit with 0.
    pub is_error: bool,
    /// The exit status, for a tool that runs a command.
    pub exit_code: Option<i32>,
}

impl ToolResult {
    /// A successful result.
    pub fn ok(output: impl Into<String>) -> Self {
        ToolResult { output: output.into(), truncated: false, is_error: false, exit_code: None }
    }

    /// A failed result: the model reads `output` and decides what to do next.
    pub fn error(output: impl Into<String>) -> Self {
        ToolResult { is_error: true, ..ToolResult::ok(output) }
    }

    /// Marks the result as failed or not.
    #[must_use]
    pub fn with_error(mut self, is_error: bool) -> Self {
        self.is_error = is_error;
        self
    }

    /// Marks the output as truncated or not.
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
}

/// Hears a call's output while it runs; the conversation turns it into coalesced
/// `ToolCallOutputUpdated` events.
pub trait ToolOutputSink: Send {
    /// The end of the output so far and the size of all of it in bytes.
    fn update(&mut self, tail: &str, bytes: u64);
}

impl<F: FnMut(&str, u64) + Send> ToolOutputSink for F {
    fn update(&mut self, tail: &str, bytes: u64) {
        self(tail, bytes);
    }
}

/// A [`ToolOutputSink`] that ignores every update.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOutput;

impl ToolOutputSink for NoOutput {
    fn update(&mut self, _tail: &str, _bytes: u64) {}
}

#[cfg(test)]
mod tests;
