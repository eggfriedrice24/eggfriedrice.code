//! `conversation.subscribe`: follow a conversation's events as they happen.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, ConversationSummary, EventEnvelope, PageCursor, Seq};

/// The params of `conversation.subscribe`, a streaming method.
///
/// With `after_seq`, the daemon replays the events after it when the gap is small (at
/// most 128 events or 1 MiB) and otherwise sends a bounded snapshot; without it, the
/// stream starts with a snapshot. Live events follow, each with a larger sequence number
/// than anything sent before. A subscriber that falls behind its bounded queue is closed
/// with `overflow` and its `last_seq`, and subscribes again from there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationSubscribe {
    /// The conversation to follow.
    pub conversation_id: ConversationId,
    /// The largest sequence number the client has already seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<Seq>,
    /// True when a person at this client can type answers to a running command that
    /// waits for input, with `input.respond`. The daemon stops a command that waits for
    /// hidden input, such as a password, when no live subscription of its conversation
    /// can answer. Absent means false, so an older client counts as one that cannot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub answers_input: bool,
}

/// One item of a `conversation.subscribe` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConversationSubscribeItem {
    /// One event, replayed or live.
    Event(EventEnvelope),
    /// A bounded view of the conversation, sent instead of a replay that would be too
    /// large.
    Snapshot(ConversationSnapshot),
}

/// A bounded view of a conversation: its summary and its most recent events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationSnapshot {
    /// The conversation as a list shows it.
    pub conversation: ConversationSummary,
    /// The most recent events, oldest first.
    pub events: Vec<EventEnvelope>,
    /// The cursor for `conversation.history` to page through the events before
    /// `events`; absent when `events` starts at the beginning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_cursor: Option<PageCursor>,
    /// The sequence number that the snapshot is complete up to. Live events after it
    /// follow.
    pub hwm: Seq,
}
