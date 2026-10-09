//! The conversation engine: one actor per conversation that queues prompts and runs
//! turns.
//!
//! - [`ConversationActor::spawn`] starts the actor of one conversation
//!   ([`ConversationStart`], a [`ConfigSource`] of [`ConversationConfig`]s,
//!   [`ConversationDeps`]) and returns
//!   its [`ConversationHandle`], the only way in: send a prompt (a second one queues
//!   behind the running turn), steer the running turn, interrupt it in two phases,
//!   answer an approval, read its [`ConversationState`].
//! - A turn assembles the request (the system prompt, every turn since the newest
//!   summary as the model read it, within the byte limit of [`HistoryLimits`], the
//!   live-state preamble regenerated from the prompt's shell context, the tool
//!   definitions), streams the provider, and records every step as an event through the
//!   store's writer. Each request starts with the request before it. Provider items in
//!   `provider_raw` go back unchanged to the provider and model that made them.
//! - Every tool call passes `turn.rs`'s `authorize_tool_call`, the single permission
//!   check point, where `efr_permissions::Engine::decide` answers Allow, Contain, Ask or
//!   Deny.
//! - The context ([`CompactionConfig`], [`ContextLimits`] and the constants beside
//!   them): the trigger, the hard cap and the estimate of the compaction contract in
//!   the README, section "Context". A turn compacts on its own at the trigger and
//!   after an overflow (`turn/compact.rs`); [`ConversationHandle::compact`] compacts
//!   between turns. Pruning, the cut, the summary request and its prompt are in
//!   `compaction.rs`, the fresh context block in `fresh.rs`.
//! - [`Toolbox`] ([`ToolCall`], [`CallContext`], [`ToolOutcome`], [`OutputSink`]): the
//!   tools as a conversation sees them; the daemon implements it over
//!   `efr_tools::ToolRegistry`.
//! - Drafts ([`ConversationDraft`], [`draft_channel`]): the text, the reasoning and the
//!   tool input of a running turn as they arrive from the model, sent at most once per
//!   [`ConversationConfig::draft_interval`] to the daemon for live clients, never stored.
//! - [`ScopeResolver`] ([`GitScopeResolver`]): the scope of each turn, derived again
//!   from the shell's working directory through `efr-scope`.
//! - The `auto` sandbox: the check point runs a contained call with no question, asks
//!   the user about each exit with its record, runs an approved exit with the narrowest
//!   launch, and asks whether to keep git settings that a call changed. An `auto` turn
//!   without a working sandbox runs as `cautious`. [`ExitJudge`] is the seam of the
//!   classifier (phase 3).
//!
//! Allowed dependencies: `efr-provider`, `efr-permissions`, `efr-scope`, `efr-store`,
//! `efr-sandbox` (only `secret_like`), `efr-protocol` and `efr-stdx`. Not `efr-tools`:
//! that crate depends on `efr-shell`, and `efr-conversation -> efr-shell` is forbidden
//! through any chain, so the tools arrive through [`Toolbox`]. What does not belong here: shells
//! (reached only through the daemon's shell tool), transports, the mapping of errors to
//! wire codes, and rendering.

mod actor;
mod approvals;
mod compaction;
mod config;
mod context;
mod drafts;
mod error;
mod exit;
mod fresh;
mod gap;
mod history;
mod interrupt;
mod judge;
mod preamble;
mod questions;
mod resolver;
mod scratch;
mod settings;
mod steer;
#[cfg(test)]
mod testing;
mod toolbox;
mod turn;

pub use actor::{ConversationActor, ConversationHandle, ConversationState, completed_result};
pub use config::{ConfigSource, ConversationConfig, ConversationDeps, ConversationStart, HostInfo};
pub use context::{
    BREAKER_TRIES, BYTES_PER_TOKEN, CompactionConfig, ContextLimits, DEFAULT_AUTO_AT,
    DEFAULT_CONTEXT_WINDOW, HARD_CAP_PERCENT, PRUNE_KEEP_TOKENS, PRUNE_MIN_TOKENS,
    PRUNED_OUTPUT_STUB, SUMMARY_MAX_OUTPUT_TOKENS, SUMMARY_REASONING_TOKENS, TAIL_TOKENS,
    estimate_tokens,
};
pub use drafts::{ConversationDraft, DRAFT_CAPACITY, draft_channel};
pub use error::ConversationError;
pub use history::HistoryLimits;
pub use judge::ExitJudge;
pub use resolver::{GitScopeResolver, ScopeResolver};
pub use toolbox::{CallContext, OutputSink, ToolCall, ToolOutcome, Toolbox};
