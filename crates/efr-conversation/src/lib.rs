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
//!   `provider_raw` go back unchanged to the provider and model that made them. A call
//!   of a tool that the request does not offer goes as text (`unoffered.rs`).
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
//!   `efr_tools::ToolRegistry`. A call reports the floor files that it changed with
//!   their backups ([`FileBackup`]), and the toolbox deletes allowed paths in the real
//!   tool caches ([`CacheDeleteOutcome`]).
//! - Drafts ([`ConversationDraft`], [`draft_channel`]): the text, the reasoning and the
//!   tool input of a running turn as they arrive from the model, sent at most once per
//!   [`ConversationConfig::draft_interval`] to the daemon for live clients, never stored.
//! - [`ScopeResolver`] ([`GitScopeResolver`]): the scope of each turn, derived again
//!   from the shell's working directory through `efr-scope`.
//! - The `auto` sandbox: the check point runs a contained call with no question, asks
//!   the user about each exit with its record, runs an approved exit with the narrowest
//!   launch, and asks whether to keep git settings that a call changed. An `auto` turn
//!   without a working sandbox runs as `cautious`. [`ExitJudge`] ([`JudgeInput`],
//!   [`JudgeOutcome`]) is the seam of the classifier (phase A), and [`PresenceReader`]
//!   ([`AlwaysPresent`]) says whether a person is there to answer a question.
//! - Background jobs (phase C): a [`ToolOutcome`] names the job that a call started,
//!   the daemon hands each job notice to [`ConversationHandle::notice`]
//!   ([`JobNoticeInput`]), and [`notice_text`] writes a notice with the line
//!   [`NOT_USER_INPUT`], escaped so that the job's output cannot end it. A notice rides
//!   in the next turn, or starts a wake turn while the conversation is idle, within
//!   the wake limits and only in `auto`. efr's own notes, such as the port forwards of
//!   a job, come from [`Toolbox::take_notes`] and go to the model before its next call,
//!   also in a running turn, after the line [`NOT_USER_NOTE`].
//! - The index redaction ([`redact_for_index`], [`REDACTION_VERSION`]): the text that
//!   efrd's search index keeps, with each secret that the preamble's line redaction
//!   or a pass over config text and private key blocks recognises replaced by `[***]`.
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
mod index_text;
mod interrupt;
mod judge;
mod notice;
mod preamble;
mod presence;
mod questions;
mod resolver;
mod scratch;
mod settings;
mod steer;
#[cfg(test)]
mod testing;
mod toolbox;
mod turn;
mod unoffered;
mod wake;

pub use actor::{
    ConversationActor, ConversationHandle, ConversationState, PromptRouting, completed_result,
};
pub use config::{
    AllowWriter, ConfigSource, ConversationConfig, ConversationDeps, ConversationStart,
    ForeignDefault, HostInfo,
};
pub use context::{
    BREAKER_TRIES, BYTES_PER_TOKEN, CompactionConfig, ContextLimits, DEFAULT_AUTO_AT,
    DEFAULT_CONTEXT_WINDOW, HARD_CAP_PERCENT, PRUNE_KEEP_TOKENS, PRUNE_MIN_TOKENS,
    PRUNED_OUTPUT_STUB, SUMMARY_MAX_OUTPUT_TOKENS, SUMMARY_REASONING_TOKENS, TAIL_TOKENS,
    estimate_tokens,
};
pub use drafts::{ConversationDraft, DRAFT_CAPACITY, draft_channel};
pub use error::{ConversationError, UndoBusyReason};
pub use history::HistoryLimits;
pub use index_text::{REDACTION_VERSION, redact_for_index};
pub use judge::{ExitJudge, JudgeInput, JudgeOutcome};
pub use notice::{
    JobNoticeInput, NOT_USER_INPUT, NOT_USER_NOTE, NOTICE_LINE_BYTES, NOTICE_TAIL_BYTES,
    NOTICE_TAIL_LINES, NoticeBody, job_lifetime_fact, notice_text, notice_text_refused,
};
pub use presence::{AlwaysPresent, PresenceReader};
pub use resolver::{GitScopeResolver, ScopeResolver};
pub use scratch::scratch_dirs;
pub use toolbox::{
    CacheDeleteOutcome, CallContext, FileBackup, OutputSink, SnapshotRecord, ToolCall, ToolOutcome,
    Toolbox, TurnChanges, UndoCandidate, UndoRequest,
};
