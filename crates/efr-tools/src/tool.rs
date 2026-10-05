//! The tool boundary: what a tool tells the model, what a call needs, what it returns.

use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use efr_protocol::InputWait;
use efr_scope::Home;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{ToolContext, ToolError, paths};

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
    /// it resolves paths lexically against the context and touches no file; the caller
    /// adds what they reach through symbolic links with
    /// [`ToolRequirements::with_real_paths`].
    fn requirements(&self, ctx: &ToolContext, input: &Value)
    -> Result<ToolRequirements, ToolError>;

    /// What a call with `input` would change, for the user who approves it, such as a
    /// diff of the file a write replaces. It reads but never changes anything; a call
    /// that cannot be previewed has none, and so does every tool by default.
    async fn preview(&self, _ctx: &ToolContext, _input: &Value) -> Option<String> {
        None
    }

    /// True when a call with `input` takes an input that the user chooses to type
    /// while it reports no wait, as a command that prints nothing for a while may need.
    /// Pure, like [`requirements`](Tool::requirements). No tool takes one by default.
    fn takes_manual_input(&self, _input: &Value) -> bool {
        false
    }

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
    /// The call reads the path and may read anything below it, as a recursive search
    /// or a glob does.
    ReadTree,
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
    /// The directory the command line starts in.
    pub command_dir: Option<PathBuf>,
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

    /// Adds a directory the call reads with everything below it.
    #[must_use]
    pub fn with_read_tree(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(PathAccess { path: path.into(), mode: AccessMode::ReadTree });
        self
    }

    /// Adds a path the call writes.
    #[must_use]
    pub fn with_write(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(PathAccess { path: path.into(), mode: AccessMode::Write });
        self
    }

    /// Adds, after each declared path that reaches the file system through a symbolic
    /// link, the path it reaches, with the same access, so the permission engine judges
    /// both: `cat notes` where `notes` links to `~/.ssh/id_ed25519` also declares the
    /// key, and the root of a recursive search that is a link also declares its target.
    /// A home reached through a link is not added, because the engine knows both forms
    /// of it, and nothing is added for a path the file system cannot resolve.
    ///
    /// The paths come from [`Tool::requirements`] lexically, so this is the one step
    /// that reads the file system. It blocks; async callers run it in `spawn_blocking`.
    #[must_use]
    pub fn with_real_paths(mut self, home: &Home) -> Self {
        let mut paths: Vec<PathAccess> = Vec::with_capacity(self.paths.len());
        for access in self.paths {
            let real =
                if access.path.is_absolute() { paths::real_form(home, &access.path) } else { None };
            let mode = access.mode;
            paths.push(access);
            if let Some(real) = real
                && !paths.iter().any(|known| known.path == real && known.mode == mode)
            {
                paths.push(PathAccess { path: real, mode });
            }
        }
        self.paths = paths;
        self
    }

    /// Sets the command line the call runs.
    #[must_use]
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// Sets the directory the command line starts in.
    #[must_use]
    pub fn with_command_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.command_dir = Some(dir.into());
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
            .field("command_dir", &self.command_dir)
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

/// Hears a call's output while it runs, and whether its command waits for input; the
/// conversation turns them into coalesced `ToolCallOutputUpdated` events and
/// `ToolCallInputChanged` events.
pub trait ToolOutputSink: Send {
    /// The end of the output so far and the size of all of it in bytes.
    fn update(&mut self, tail: &str, bytes: u64);

    /// The call's command started or stopped waiting for input; each change comes
    /// once. `looks_secret` is true for a visible wait whose prompt reads like a
    /// password prompt behind a relay, where the program on the inner terminal decides
    /// whether the answer is shown. Ignored by default.
    fn input_changed(&mut self, _wait: InputWait, _looks_secret: bool) {}

    /// Whether a person can answer hidden input, such as a password, for this call
    /// now. Asked when the command starts to wait for hidden input and again while it
    /// waits; `false` stops the command. True by default, which lets it wait until it
    /// ends or the call's timeout passes.
    fn can_answer_hidden(&mut self) -> bool {
        true
    }

    /// Whether a person who can type answers follows the call now, which keeps a call
    /// with an [`interactive_limit`](crate::ToolContext::interactive_limit) running past
    /// its timeout. False by default, which keeps the timeout.
    fn can_answer(&mut self) -> bool {
        false
    }
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
