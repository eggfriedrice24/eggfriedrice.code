//! `conversation.subscribe`: follow a conversation's events as they happen.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    CompactionTrigger, ContextUse, ConversationId, ConversationSummary, EventEnvelope, PageCursor,
    Seq, TurnId,
};

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
    /// True when the client wants [`Draft`] items: the text, the reasoning and the
    /// tool input of a running turn as they arrive from the model, before the event
    /// log has them. Absent means false, so an older client gets no drafts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drafts: bool,
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
    /// What a running turn got from the model since the last draft. Only a subscriber
    /// that asked for `drafts` gets it. It is not an event: it has no sequence number
    /// of its own, and the daemon does not store it.
    Draft(Draft),
}

/// A draft: part of a running turn as it arrives from the model, before the event log
/// has it.
///
/// Drafts are best effort. The daemon sends at most one draft of each part per
/// `conversation.draft_interval_ms` (the first one at once). When the subscriber's
/// queue is full, it merges the drafts of each part that wait and sends them at the
/// next free place (the capability `drafts_coalesced`), so a slow subscriber gets
/// fewer, larger drafts with no gap. It never closes a subscription because of a draft.
/// The events stay the source of truth: `assistant_message_updated` and
/// `assistant_message_completed` carry the same text at the same offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Draft {
    /// The turn that the draft belongs to.
    pub turn_id: TurnId,
    /// The sequence number of the last event that the turn recorded before it made
    /// this draft. Every event of the stream at or below it comes before the draft. An
    /// event of the turn with a larger number came after the draft. The daemon does
    /// not send a draft that is older than an `assistant_message_completed`, a
    /// `tool_call_started` or the end of a turn that it sent on the stream before.
    pub after_seq: Seq,
    /// What arrived.
    pub draft: DraftPart,
}

/// The part of a running turn that a [`Draft`] carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DraftPart {
    /// More text of an assistant message. `index`, `offset` and `delta` have the
    /// meaning that they have in `assistant_message_updated`: `offset` is the byte
    /// offset of `delta` in the whole text of message `index`. Drafts and updates count
    /// the same text, so a client can merge the two.
    Text {
        /// The position of the message among the assistant messages of the turn that
        /// have text, from 0. A model answer without text takes no position.
        index: u32,
        /// The offset of `delta` in the text of the message, in bytes of UTF-8. It is
        /// not a count of characters or of UTF-16 units: a JavaScript client compares
        /// it with the UTF-8 length of its text, never with `text.length`.
        offset: u64,
        /// The text after `offset`.
        delta: String,
    },
    /// More of the reasoning summary of the model. Reasoning never goes into the event
    /// log.
    Reasoning {
        /// The offset of `delta` in the reasoning text of the whole turn, in bytes of
        /// UTF-8. The reasoning of a later model call in the same turn continues the
        /// text after a blank line.
        offset: u64,
        /// The reasoning text after `offset`.
        delta: String,
        /// The title of the newest reasoning section: the last line of the reasoning
        /// so far that is all bold (`**Title**`), without the stars. Absent until
        /// such a line comes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// The model writes the input of a tool call. `tool_call_started` comes only
    /// after the whole answer of the model is in.
    ToolInput {
        /// The position of the call among the tool calls of this answer of the model,
        /// from 0, so a client can tell two calls of one answer apart.
        call: u32,
        /// The name of the tool.
        tool: String,
        /// How many bytes of input the model has written so far: of the JSON
        /// arguments, or of the text of a freeform tool such as `apply_patch`.
        bytes: u64,
    },
    /// How full the model's context is. The turn sends it before each model call, with
    /// its estimate of the request, and after each call, with the count that the
    /// provider reported. A client shows it as `ctx N%` while the turn runs.
    Context(ContextUse),
    /// The turn compacts the context before its next model call. A client shows
    /// `compacting context` until the `conversation_compacted` event, the next
    /// `context` draft or the end of the turn, whichever comes first.
    Compacting {
        /// What started the compaction: `auto` or `overflow`.
        trigger: CompactionTrigger,
    },
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
    /// What the running turn has so far, when a turn runs: the text of its current
    /// message and its reasoning, which `events` can miss. Only a daemon that announces
    /// `snapshot_live` sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveTurn>,
}

/// The state of a running turn in a [`ConversationSnapshot`]: what a client needs to
/// show the text and the reasoning so far after it joins in the middle of a message.
///
/// `text` is the whole text of message `index` so far, from its start. Drafts and
/// `assistant_message_updated` events of the same message that follow the snapshot
/// continue it: one whose `offset` is at most `text_bytes` overlaps it, and a client
/// keeps the part past `text_bytes`. `reasoning` is the reasoning of the whole turn so
/// far in the same way, for the reasoning drafts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LiveTurn {
    /// The running turn.
    pub turn_id: TurnId,
    /// The sequence number of the last event that the turn recorded before this state,
    /// with the meaning that it has in a [`Draft`].
    pub after_seq: Seq,
    /// The position of the current message among the assistant messages of the turn
    /// that have text, from 0, as in [`DraftPart::Text`].
    pub index: u32,
    /// The text of the current message so far; empty before its first text.
    pub text: String,
    /// The length of `text` in bytes of UTF-8: the offset at which the next draft or
    /// update of message `index` continues.
    pub text_bytes: u64,
    /// The reasoning of the turn so far; absent when the model sent none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// The title of the newest reasoning section, as in [`DraftPart::Reasoning`];
    /// absent until such a line comes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_title: Option<String>,
}
