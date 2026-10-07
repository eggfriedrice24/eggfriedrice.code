//! Drafts: what a running turn got from the model, for live clients only.
//!
//! A turn sends a [`ConversationDraft`] on the broadcast channel of
//! [`ConversationDeps::drafts`](crate::ConversationDeps::drafts) at most once per
//! [`ConversationConfig::draft_interval`](crate::ConversationConfig::draft_interval).
//! The daemon forwards it to the subscribers of the conversation that asked for drafts.
//! A draft never goes into the event log, so the log is the same with and without
//! subscribers. The channel carries `efr-protocol` types only, because this crate
//! must not know the transport.

use efr_protocol::{ConversationId, DraftPart, Seq, TurnId};
use tokio::sync::broadcast;

/// How many drafts the broadcast channel holds for its slowest receiver. A receiver
/// that falls further behind loses the oldest ones; a draft is best effort.
pub const DRAFT_CAPACITY: usize = 256;

/// One draft of a running turn, with the conversation it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationDraft {
    /// The conversation of the turn.
    pub conversation_id: ConversationId,
    /// The turn.
    pub turn_id: TurnId,
    /// The sequence number of the last event that the turn recorded before it made the
    /// draft. An event of the turn with a larger number came after the draft.
    pub after_seq: Seq,
    /// What arrived from the model.
    pub part: DraftPart,
}

/// A new channel for drafts with room for [`DRAFT_CAPACITY`] drafts. The daemon keeps
/// the sender in [`ConversationDeps`](crate::ConversationDeps) and subscribes one
/// receiver for each subscriber that asked for drafts.
pub fn draft_channel() -> broadcast::Sender<ConversationDraft> {
    broadcast::channel(DRAFT_CAPACITY).0
}
