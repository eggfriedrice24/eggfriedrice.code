//! The model's context: how full it is, and the compactions that make room in it.
//!
//! The rules (the trigger, the estimate, the order of pruning and summarizing, the
//! history after a compaction) are in the README of `efr-conversation`, the
//! "Context" section. This module holds only what crosses the wire.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CompactionId, TurnId, Usage};

/// How full the model's context is, against the point where efrd compacts it.
///
/// A client shows `ctx N%`, where N is `tokens` as a percent of `limit`: 100% means that
/// a compaction runs now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ContextUse {
    /// The tokens in the context: after a model call, its input plus its output; before
    /// a call, efrd's estimate of the request.
    pub tokens: u64,
    /// The tokens at which a client shows 100%: the auto compaction trigger
    /// (`compaction.auto_at` percent of `window`), or the hard cap (95% of `window`) when
    /// `compaction.auto` is off.
    pub limit: u64,
    /// The context window of the turn's model, in tokens. For a model whose window efr
    /// does not know, the default that efrd counts with.
    pub window: u64,
}

/// What started a compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionTrigger {
    /// The context reached the auto compaction trigger before a model call of a turn.
    Auto,
    /// The user asked for it: `conversation.compact` (`efr compact`, `,compact`).
    Manual,
    /// The provider refused a request as larger than the model's context window, or
    /// efrd's estimate of a request was above the hard cap.
    Overflow,
}

/// One compaction of a conversation's context, as `conversation_compacted` records it.
///
/// The compaction covers the model's history up to a cut: every message of the turns
/// before `through_turn`, and the messages of `through_turn` before `through_message`
/// (all of them when `through_message` is absent). The messages after the cut stay
/// verbatim. With a `summary`, the summary replaces everything before the cut. Without
/// one (only pruning ran), every tool result before the cut whose output is longer
/// than the stub reads the stub, and the summary of an earlier compaction stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Compaction {
    /// The compaction.
    pub compaction_id: CompactionId,
    /// The turn that ran it, for an auto or an overflow compaction inside a turn. Absent
    /// for a manual compaction, which runs between turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// What started it.
    pub trigger: CompactionTrigger,
    /// What the user asked the summary to keep, for a manual compaction that named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    /// The model that wrote the summary: the model of the conversation's turn.
    pub model: String,
    /// The context window of that model, in tokens.
    pub window: u64,
    /// The tokens at which a client shows 100%, as [`ContextUse::limit`].
    pub limit: u64,
    /// The tokens in the context before the compaction: the newest real count plus the
    /// estimate of what came after it, or the size that the provider refused.
    pub tokens_before: u64,
    /// The estimated tokens in the context after the compaction.
    pub tokens_after: u64,
    /// The newest turn whose messages the compaction covers, in full or in part.
    pub through_turn: TurnId,
    /// When present, the compaction covers only the messages of `through_turn` before
    /// this index (0 is the prompt), and the verbatim tail starts at this message of the
    /// same turn. A cut never falls between a tool call and its result. Absent: the
    /// compaction covers the whole turn, and the tail starts with the next turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_message: Option<u32>,
    /// The turns that the verbatim tail keeps, in full or in part.
    pub kept_turns: u32,
    /// The tool results whose output the pruning replaced with the stub. Absent when
    /// none.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub pruned_outputs: u32,
    /// The estimated tokens that the pruning freed. Absent when none.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub pruned_tokens: u64,
    /// The earlier turns that the history had left out before the compaction (the
    /// safety net of the history limits), so the summary never saw them. Absent when
    /// none.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub omitted_turns: u32,
    /// The oldest messages before the cut that the summary request left out, because
    /// they did not fit in the model's context, so the summary never saw them. Absent
    /// when none.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub omitted_messages: u32,
    /// The summary that replaces the history before the cut. Absent when pruning alone
    /// freed enough room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The tokens that the summary request used, when the provider reported them. Its
    /// `output_tokens` is the size of the summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

// NOTE: serde's `skip_serializing_if` passes a reference.
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

pub(crate) fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}
