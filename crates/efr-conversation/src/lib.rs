//! The conversation engine: one actor per conversation that queues prompts and runs
//! turns.
//!
//! - [`ConversationActor::spawn`] starts the actor of one conversation
//!   ([`ConversationStart`], a [`ConfigSource`] of [`ConversationConfig`]s,
//!   [`ConversationDeps`]) and returns
//!   its [`ConversationHandle`], the only way in: send a prompt (a second one queues
//!   behind the running turn), steer the running turn, interrupt it in two phases,
//!   answer an approval, read its [`ConversationState`].
//! - A turn assembles the request (the system prompt, bounded history from the event
//!   log within [`HistoryLimits`], the live-state preamble regenerated from the prompt's
//!   shell context, the tool definitions), streams the provider, and records every step
//!   as an event through the store's writer. Provider items in `provider_raw` go back
//!   unchanged to the provider and model that made them.
//! - Every tool call passes `turn.rs`'s `authorize_tool_call`, the single permission
//!   check point, where `efr_permissions::Engine::decide` answers Allow, Ask or Deny.
//! - [`Toolbox`] ([`ToolCall`], [`CallContext`], [`ToolOutcome`], [`OutputSink`]): the
//!   tools as a conversation sees them; the daemon implements it over
//!   `efr_tools::ToolRegistry`.
//! - [`ScopeResolver`] ([`GitScopeResolver`]): the scope of each turn, derived again
//!   from the shell's working directory through `efr-scope`.
//!
//! Allowed dependencies: `efr-provider`, `efr-permissions`, `efr-scope`, `efr-store`,
//! `efr-protocol` and `efr-stdx`. The allowlist also names `efr-tools`, but that crate
//! depends on `efr-shell`, and `efr-conversation -> efr-shell` is forbidden through any
//! chain, so the tools arrive through [`Toolbox`]. What does not belong here: shells
//! (reached only through the daemon's shell tool), transports, the mapping of errors to
//! wire codes, and rendering.

mod actor;
mod approvals;
mod config;
mod error;
mod history;
mod interrupt;
mod preamble;
mod resolver;
mod scratch;
mod steer;
#[cfg(test)]
mod testing;
mod toolbox;
mod turn;

pub use actor::{ConversationActor, ConversationHandle, ConversationState};
pub use config::{ConfigSource, ConversationConfig, ConversationDeps, ConversationStart, HostInfo};
pub use error::ConversationError;
pub use history::HistoryLimits;
pub use resolver::{GitScopeResolver, ScopeResolver};
pub use toolbox::{CallContext, OutputSink, ToolCall, ToolOutcome, Toolbox};
