//! `conversations.recent`: the recent conversations for `,resume` and `efr resume`, the
//! ones of the current scope first.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, ConversationSummary, RouteLabel, RouteScope};

/// The params of `conversations.recent`, a read method.
///
/// efrd lists the conversations whose scope key is the key of `pwd` first, newest
/// first by their last activity, then all others, newest first. Only efrd knows the
/// key of a directory and which terminal holds a conversation, so the order is
/// efrd's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsRecent {
    /// The terminal that asks, such as `/dev/pts/3`: its conversations show as `here`,
    /// and `current` names its current one. Absent for a client without a terminal,
    /// such as the phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The working directory that decides which scope comes first. Absent: newest
    /// first only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pwd: Option<PathBuf>,
    /// The most conversations to return; efrd picks a default of 10 and a maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The result of `conversations.recent`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsRecentResult {
    /// The scope of `pwd`, whose conversations come first. Absent without a `pwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RouteLabel>,
    /// The current conversation of the terminal: the one of its last prompt, `,new`
    /// or resume. Absent without a `tty`, or when the terminal holds none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<ConversationId>,
    /// The conversations, the ones of the scope of `pwd` first, each part newest first.
    pub conversations: Vec<RecentConversation>,
}

/// One conversation of the list of `conversations.recent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecentConversation {
    /// What a list shows about the conversation: its id, title, status and last
    /// activity.
    pub summary: ConversationSummary,
    /// Its scope key.
    pub route: RouteScope,
    /// The name of its project. Absent outside projects and for a project that is no
    /// longer registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Its prompt turns: a turn that a job notice started and a prompt that was taken
    /// back before it ran do not count.
    pub turns: u32,
    /// Another open terminal that holds it, such as `/dev/pts/4`: one of its slots
    /// holds it while its shell runs, or it follows the conversation. A phone never
    /// holds one. Absent when no other terminal holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_in: Option<String>,
    /// True when it is a conversation of the terminal that asks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub here: bool,
}
