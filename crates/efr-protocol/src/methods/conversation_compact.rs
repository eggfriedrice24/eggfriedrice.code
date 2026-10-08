//! `conversation.compact`: compact a conversation's context now.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, Compaction, ConversationId, Seq};

/// The params of `conversation.compact` (`efr compact [focus]`, `,compact [focus]`).
///
/// efrd compacts the context of an idle conversation: it prunes old tool output and,
/// when that does not free enough room, writes a summary of the history before the
/// verbatim tail. It records `conversation_compacted` and answers when the compaction
/// is done. It never starts a turn.
///
/// Refusals, each with nothing recorded:
///
/// - `conflict` while a turn of the conversation runs: the turn compacts on its own
///   when it needs to, so efrd does not wait for it;
/// - `conflict` when nothing lies before the verbatim tail, so a compaction would free
///   no room;
/// - `not_found` for a conversation that does not exist.
///
/// A prompt that arrives while the compaction runs waits in the queue and starts after
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationCompact {
    /// Makes the request idempotent: a retry with the same id returns the first result.
    pub command_id: CommandId,
    /// The conversation to compact.
    pub conversation_id: ConversationId,
    /// What the summary must keep, in the user's words, such as `the failing test and
    /// its fix`. Absent: the summary keeps what its fixed sections ask for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

/// The result of `conversation.compact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationCompactResult {
    /// The sequence number of the `conversation_compacted` event.
    pub seq: Seq,
    /// The compaction, as the event records it.
    pub compaction: Compaction,
}
