//! `conversations.watch`: the rows of the recent conversations of the whole machine,
//! then each change of a row, for the lists of the phone.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, RecentConversation};

/// The params of `conversations.watch`, a streaming read method.
///
/// The stream starts with a `snapshot`: the recent conversations of the machine, newest
/// first by their last activity, as `conversations.recent` without a `tty` and a `pwd`
/// gives them. Then efrd sends `changed` each time a commit changes what a list shows
/// of a conversation: its title, its status, its working directory, its scope, its
/// project or its count of turns. A commit that changes only the time and the sequence
/// number of the last activity sends nothing. A conversation that is not in the
/// snapshot gets `changed` at its first such commit. `removed` follows the delete of a
/// conversation.
///
/// A subscriber that falls behind is closed with `overflow`. It then watches again and
/// gets a new snapshot. The `last_seq` of that error names the last commit that the
/// stream looked at, and a new watch does not use it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsWatch {
    /// The most conversations of the snapshot; efrd picks a default of 20 and a
    /// maximum of 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One item of a `conversations.watch` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConversationsWatchItem {
    /// The recent conversations, newest first. Always the first item.
    Snapshot {
        /// The rows, as `conversations.recent` gives them. `here` is false and
        /// `open_in` names a terminal that held the conversation when efrd read it.
        conversations: Vec<RecentConversation>,
    },
    /// What a list shows of one conversation changed, or the stream did not show it
    /// before. The row replaces the row with the same id.
    Changed {
        /// The row now. Boxed, so the other items stay small.
        conversation: Box<RecentConversation>,
    },
    /// The conversation was deleted. A list drops its row.
    Removed {
        /// The deleted conversation.
        conversation_id: ConversationId,
    },
}
