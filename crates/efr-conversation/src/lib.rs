//! The conversation engine: one actor per conversation that queues prompts and runs
//! turns. This crate is built up module by module; the actor and the turn loop land
//! last.
//!
//! Allowed dependencies: `efr-provider`, `efr-permissions`, `efr-scope`, `efr-store`,
//! `efr-protocol` and `efr-stdx`. The allowlist also names `efr-tools`, but that crate
//! depends on `efr-shell`, and `efr-conversation -> efr-shell` is forbidden through any
//! chain, so the tools arrive through [`Toolbox`]. What does not belong here: shells
//! (reached only through the daemon's shell tool), transports, the mapping of errors to
//! wire codes, and rendering.

mod approvals;
mod error;
mod history;
mod interrupt;
mod preamble;
mod resolver;
mod scratch;
mod steer;
mod toolbox;

pub use error::ConversationError;
pub use history::HistoryLimits;
pub use resolver::{GitScopeResolver, ScopeResolver};
pub use toolbox::{CallContext, OutputSink, ToolCall, ToolOutcome, Toolbox};
