//! The tools the model calls.
//!
//! - [`Tool`]: the trait, with [`spec`](Tool::spec) ([`ToolSpec`]: name,
//!   description, JSON Schema from schemars), [`requirements`](Tool::requirements)
//!   ([`ToolRequirements`]: every path with its [`AccessMode`], the command line,
//!   network, interactive) and [`invoke`](Tool::invoke) ([`ToolResult`], with live
//!   output to a [`ToolOutputSink`]).
//! - [`ToolRegistry`]: the specs for the provider request and dispatch by name; it
//!   never hands out a tool.
//! - [`ShellTool`] over `efr_shell::CommandRunner`, [`ReadFileTool`] and
//!   [`WriteFileTool`], which records each original in a [`WriteJournal`]
//!   ([`JournalEntry`], [`FileSnapshot`], [`Original`]) before it writes.
//! - [`ToolContext`] and [`CallIds`]: where a call runs.
//! - [`truncate_middle`]: head and tail with a marker, [`DEFAULT_OUTPUT_LIMIT`]
//!   (32 KiB) unless a tool says otherwise.
//!
//! Allowed dependencies: `efr-shell`, `efr-scope` (the home directory), `efr-protocol`
//! and `efr-stdx`. What does not belong here: permissions. Tools declare, the
//! permission engine (`efr-permissions`) decides, and `efr-conversation` enforces, at
//! its single check point; `xtask/src/deps.rs` forbids `efr-tools -> efr-permissions`.

mod context;
mod error;
mod journal;
#[cfg(test)]
mod testing;
mod tool;

pub use context::{CallIds, ToolContext};
pub use error::ToolError;
pub use journal::{FileSnapshot, JournalEntry, MemoryJournal, Original, WriteJournal};
pub use tool::{
    AccessMode, NoOutput, PathAccess, Tool, ToolOutputSink, ToolRequirements, ToolResult, ToolSpec,
};
