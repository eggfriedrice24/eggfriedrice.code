//! `conversation.search`: the places in one conversation that match some words, for
//! the preview of the picker of `,resume` and for the phone.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, SearchHit, SearchLine, SearchPartial};

/// The params of `conversation.search`, a read method for user-side peers: the hits in
/// one conversation.
///
/// efrd reads `query` as `conversations.search` does. `not_found` for a conversation
/// that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationSearch {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The words to find. Empty after a trim: the newest prompts and the start of the
    /// newest answer, with no ranges.
    pub query: String,
    /// The most hits to return: 20 when absent, 50 at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The result of `conversation.search`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationSearchResult {
    /// The title with its secrets replaced, and the parts that match. Absent before
    /// efrd indexed the conversation, and for a conversation without a title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<SearchLine>,
    /// The hits, newest first: at most 12 lines each, and a line of at most 400
    /// characters, which ends with `...` when efrd cut it.
    pub hits: Vec<SearchHit>,
    /// Why the search stopped before it saw every match. Absent: it saw every match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<SearchPartial>,
}
