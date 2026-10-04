//! `conversation.history`: page backwards through a conversation's events.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, EventEnvelope, PageCursor};

/// The params of `conversation.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationHistory {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// Where to continue: a `next_cursor` or a snapshot's `history_cursor`. Absent for
    /// the most recent page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<PageCursor>,
    /// The most events to return; the daemon picks a default and a maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The result of `conversation.history`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationHistoryResult {
    /// One page of events, oldest first.
    pub events: Vec<EventEnvelope>,
    /// The cursor for the page of older events; absent when this page reaches the first
    /// event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<PageCursor>,
}
