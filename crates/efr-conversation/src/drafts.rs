//! Drafts: what a running turn got from the model, for live clients only.
//!
//! A turn sends a [`ConversationDraft`] on the broadcast channel of
//! [`ConversationDeps::drafts`](crate::ConversationDeps::drafts) at most once per
//! [`ConversationConfig::draft_interval`](crate::ConversationConfig::draft_interval).
//! The daemon forwards it to the subscribers of the conversation that asked for drafts.
//! A draft never goes into the event log, so the log is the same with and without
//! subscribers. The channel carries `efr-protocol` types only, because this crate
//! must not know the transport.

use std::sync::{Mutex, PoisonError};

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

/// The status of a conversation's running turn that a client which attaches while the
/// turn runs must see at once, not only at the next draft: the newest `context` draft,
/// and the `compacting` draft while a compaction runs. The other drafts are lost to a
/// late listener, as before.
#[derive(Debug, Default)]
pub(crate) struct LiveStatus {
    held: Mutex<Held>,
}

#[derive(Debug, Default)]
struct Held {
    context: Option<ConversationDraft>,
    compacting: Option<ConversationDraft>,
}

impl LiveStatus {
    /// Keeps `draft` when it is part of the status. A `context` draft ends a
    /// compaction, as it does for a client.
    pub(crate) fn hold(&self, draft: &ConversationDraft) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        match draft.part {
            DraftPart::Context(_) => {
                held.context = Some(draft.clone());
                held.compacting = None;
            }
            DraftPart::Compacting { .. } => held.compacting = Some(draft.clone()),
            _ => {}
        }
    }

    /// The turn ended, or a compaction did: `compacting` only, or the whole status.
    pub(crate) fn clear(&self, compacting_only: bool) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.compacting = None;
        if !compacting_only {
            held.context = None;
        }
    }

    /// The drafts of the status, in the order a client must apply them.
    pub(crate) fn drafts(&self) -> Vec<ConversationDraft> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.context.iter().chain(held.compacting.iter()).cloned().collect()
    }
}
