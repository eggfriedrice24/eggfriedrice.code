//! The tools the model calls.
//!
//! - [`Tool`]: the trait, with [`spec`](Tool::spec) ([`ToolSpec`]: name,
//!   description, JSON Schema from schemars, or a [`ToolGrammar`] for a freeform tool
//!   whose input is text, read with [`freeform_text`]),
//!   [`requirements`](Tool::requirements)
//!   ([`ToolRequirements`]: every path with its [`AccessMode`], the command line,
//!   network, interactive) and [`invoke`](Tool::invoke) ([`ToolResult`], with live
//!   output to a [`ToolOutputSink`]).
//! - [`ToolRegistry`]: the specs for the provider request and dispatch by name; it
//!   never hands out a tool.
//! - [`ShellTool`] over `efr_shell::CommandRunner`, [`ReadFileTool`] and
//!   [`WriteFileTool`], which records each original in a [`WriteJournal`]
//!   ([`JournalEntry`], [`FileSnapshot`], [`Original`]) before it writes, and
//!   previews a write as a bounded unified diff for its approval
//!   ([`Tool::preview`], [`unified_diff`], which the daemon's settings tool shows
//!   too). A write reports the files it changed ([`WrittenFile`], [`WrittenKind`])
//!   with their diffs and line counts ([`written_diff`], [`WrittenDiff`]), which the
//!   call shows after it ran.
//! - [`ApplyPatchTool`]: edits files with a patch on the engine of `efr-patch`, all or
//!   nothing, and marks a delete or a move as destructive in its requirements.
//! - [`ToolContext`] and [`CallIds`]: where a call runs.
//! - [`truncate_middle`]: head and tail with a marker, [`DEFAULT_OUTPUT_LIMIT`]
//!   (32 KiB) unless a tool says otherwise.
//!
//! Allowed dependencies: `efr-shell`, `efr-scope` (the home directory), `efr-patch`
//! (the patch engine), `efr-protocol` and `efr-stdx`. What does not belong here: permissions. Tools declare, the
//! permission engine (`efr-permissions`) decides, and `efr-conversation` enforces, at
//! its single check point; `xtask/src/deps.rs` forbids `efr-tools -> efr-permissions`.

mod apply_patch;
mod context;
mod diff;
mod error;
mod journal;
mod output;
mod paths;
mod read_file;
mod registry;
mod shell_tool;
#[cfg(test)]
mod testing;
mod tool;
mod write_file;

pub use apply_patch::ApplyPatchTool;
pub use context::{CallIds, ToolContext};
pub use diff::{MAX_WRITTEN_BYTES, WrittenDiff, unified_diff, written_diff};
pub use error::ToolError;
pub use journal::{FileSnapshot, JournalEntry, MemoryJournal, Original, WriteJournal};
pub use output::{DEFAULT_OUTPUT_LIMIT, Truncated, truncate_middle};
pub use read_file::ReadFileTool;
pub use registry::ToolRegistry;
pub use shell_tool::{ShellTool, not_ready_message};
pub use tool::{
    AccessMode, FREEFORM_INPUT, NoOutput, PathAccess, Tool, ToolGrammar, ToolOutputSink,
    ToolRequirements, ToolResult, ToolSpec, WrittenFile, WrittenKind, freeform_text,
};
pub use write_file::WriteFileTool;
