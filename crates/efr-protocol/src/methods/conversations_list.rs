//! `conversations.list`: page through the conversations, newest first.

use std::path::PathBuf;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, PageCursor, Scope, Seq};

/// The params of `conversations.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsList {
    /// Where to continue: the `next_cursor` of the previous page. Absent for the first
    /// page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<PageCursor>,
    /// The most conversations to return; the daemon picks a default and a maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The result of `conversations.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsListResult {
    /// One page of conversations, the most recently updated first.
    pub conversations: Vec<ConversationSummary>,
    /// The cursor for the next page; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<PageCursor>,
}

/// What a list shows about one conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationSummary {
    /// The conversation.
    pub id: ConversationId,
    /// A short title, taken from the first prompt until something better exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// What the conversation is doing now.
    pub status: ConversationStatus,
    /// When the conversation began.
    pub created_at: Timestamp,
    /// When its last event was recorded.
    pub updated_at: Timestamp,
    /// The sequence number of its last event.
    pub last_seq: Seq,
    /// The user's working directory at the last prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The scope of the last turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// The terminal the conversation is active in, when it is the active conversation of
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
}

/// What a conversation is doing.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConversationStatus {
    /// No turn is running.
    Idle,
    /// A turn is running.
    Running,
    /// A turn waits for the user to answer an approval request.
    AwaitingApproval,
}
