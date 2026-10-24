//! `conversation.delete`: delete a conversation and everything that efr keeps of it,
//! for `efr conversation delete` and the delete key of the `,resume` picker.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId};

/// The params of `conversation.delete`, a terminal method.
///
/// efrd stops the conversation's background jobs and waits for their end, closes its
/// hidden shell, then records `conversation_deleted` with the receipt. From that event
/// on, no list, search or read shows the conversation, and the store refuses every new
/// event of it. Then efrd removes its events, its search index rows, its saved
/// messages, its receipts, its recordings, its `$SCRATCH`, its sandbox dirs, its
/// snapshot refs and its undo trash. A stop of efrd in the middle leaves no part of the
/// conversation visible, and the next start of efrd finishes the removal.
///
/// Refusals: `busy` while a turn runs, a prompt waits in its queue, a compaction or an
/// undo runs, unless `stop` is set; `not_found` for a conversation that does not exist
/// or that is deleted already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationDelete {
    /// Makes the delete idempotent.
    pub command_id: CommandId,
    /// The conversation.
    pub conversation_id: ConversationId,
    /// Delete the conversation also when another open terminal holds it. Without it,
    /// efrd deletes nothing and answers `held`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
    /// Stop the running turn, the queued prompts, a compaction or an undo first.
    /// Without it, efrd deletes nothing and answers `busy` while one of them runs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop: bool,
}

/// The result of `conversation.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConversationDeleteResult {
    /// The conversation is deleted.
    Deleted {
        /// The conversation.
        conversation_id: ConversationId,
        /// Its title, for the line of the client.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// The background jobs that efrd stopped first. Absent when none.
        #[serde(default, skip_serializing_if = "is_zero")]
        jobs_stopped: u32,
        /// The files and dirs of the conversation that efrd could not remove. The next
        /// start of efrd tries again. Absent when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        not_removed: Vec<PathBuf>,
    },
    /// Another open terminal holds the conversation, and `force` was not set: nothing
    /// changed.
    Held {
        /// That terminal.
        tty: String,
        /// The conversation's title, for the question.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// An answer from a newer daemon.
    #[serde(other)]
    Unknown,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}
