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
    /// Only the conversations of this terminal, its current one first: the one of its
    /// last prompt, `,new` or resume. One page holds them all, with no cursor. A daemon
    /// from before routing by scope ignores it and lists every conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
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
    /// Who can act on the conversation now, when it needs someone or its last turn
    /// failed. Absent when none of these holds, and from an efrd before 0.0.12.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
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
    /// The terminal that holds the conversation, when one does: it is that terminal's
    /// conversation for its scope.
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

/// Who can act on a conversation, next to its [`ConversationStatus`].
///
/// It is a member of its own, not a value of `status`: a released client reads
/// `status` with a closed set of values, so a new value there would fail its whole
/// list. A value that this build does not know reads as `unknown`, which a client
/// reads as no attention.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Attention {
    /// A question of the model waits for an answer (status `running`).
    Answer,
    /// Only the user's own machine can answer what waits: an approval that only the
    /// terminal may give, or a command that waits for input, and nothing waits that
    /// a phone can answer (status `awaiting_approval` or `running`).
    Terminal,
    /// No turn runs, and the last turn that ended failed (status `idle`).
    Failed,
    /// A value that this build does not know.
    #[serde(other)]
    Unknown,
}

impl Attention {
    /// Every value, in the order of the enum.
    pub const ALL: [Attention; 4] =
        [Attention::Answer, Attention::Terminal, Attention::Failed, Attention::Unknown];

    /// The wire form, such as `answer`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Attention::Answer => "answer",
            Attention::Terminal => "terminal",
            Attention::Failed => "failed",
            Attention::Unknown => "unknown",
        }
    }
}
