//! Drafts: what a running turn got from the model, for live clients only.
//!
//! A turn sends a [`ConversationDraft`] on the broadcast channel of
//! [`ConversationDeps::drafts`](crate::ConversationDeps::drafts) at most once per
//! [`ConversationConfig::draft_interval`](crate::ConversationConfig::draft_interval).
//! The daemon forwards it to the subscribers of the conversation that asked for drafts.
//! A draft never goes into the event log, so the log is the same with and without
//! subscribers. The channel carries `efr-protocol` types only, because this crate
//! must not know the transport.
//!
//! A turn also keeps what its current model call streamed so far in a shared holder
//! ([`LiveTurn`], read with `ConversationHandle::live_turn`). A subscriber that lost
//! drafts builds them again from it, and a client that attaches during a model call
//! can show the text and the reasoning so far, which the log does not have yet.

use std::sync::{Mutex, MutexGuard, PoisonError};

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

/// What the current model call of a running turn streamed so far: the parts that the
/// drafts carry, whole. It is the state of the turn's drafter, so a draft of the turn
/// that came later than [`after_seq`](Self::after_seq) never holds less.
///
/// All offsets and lengths count UTF-8 bytes, as the drafts and the
/// `assistant_message_updated` events do: `text.len()` is the byte length of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTurn {
    /// The running turn.
    pub turn_id: TurnId,
    /// The sequence number of the last event that the turn recorded. A draft with the
    /// same `after_seq` shows nothing that this state lacks.
    pub after_seq: Seq,
    /// The index of the assistant message that streams, as in
    /// `assistant_message_updated`. After `assistant_message_completed` of that message
    /// it is the index of the next one, with an empty `text`.
    pub index: u32,
    /// The text of message `index` so far, with its blocks joined as the completed text
    /// joins them. Empty once the message completed.
    pub text: String,
    /// The reasoning of the whole turn so far, as the `reasoning` drafts carry it.
    pub reasoning: String,
    /// The title of the newest reasoning section of the current model call (its last
    /// all-bold line), as the newest `reasoning` draft carries it.
    pub title: Option<String>,
    /// The input of each tool call of the current answer so far, in the order they
    /// started.
    pub tools: Vec<LiveToolInput>,
}

impl LiveTurn {
    /// The byte length of [`text`](Self::text), the `offset` that the next text draft
    /// of message [`index`](Self::index) starts at.
    pub fn text_bytes(&self) -> u64 {
        self.text.len() as u64
    }
}

/// The input of one tool call of the current answer, as the `tool_input` draft
/// carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveToolInput {
    /// The position of the call in the answer.
    pub call: u32,
    /// The tool.
    pub tool: String,
    /// How many bytes of input the model wrote so far.
    pub bytes: u64,
}

/// The parts of the current model call, as the drafter writes them.
#[derive(Debug)]
pub(crate) struct TurnParts {
    pub(crate) turn_id: TurnId,
    pub(crate) after_seq: Seq,
    pub(crate) index: u32,
    pub(crate) text: String,
    pub(crate) reasoning: String,
    /// Where the reasoning of the current model call starts in `reasoning`: the title
    /// comes from there on only.
    pub(crate) call_reasoning: usize,
    pub(crate) tools: Vec<LiveToolInput>,
}

impl TurnParts {
    pub(crate) fn new(turn_id: TurnId, after_seq: Seq, index: u32) -> Self {
        TurnParts {
            turn_id,
            after_seq,
            index,
            text: String::new(),
            reasoning: String::new(),
            call_reasoning: 0,
            tools: Vec::new(),
        }
    }

    fn live(&self) -> LiveTurn {
        let call = self.reasoning.get(self.call_reasoning..).unwrap_or_default();
        LiveTurn {
            turn_id: self.turn_id,
            after_seq: self.after_seq,
            index: self.index,
            text: self.text.clone(),
            reasoning: self.reasoning.clone(),
            title: call.rsplit('\n').find_map(bold_title).map(str::to_owned),
            tools: self.tools.clone(),
        }
    }
}

/// The text of `line` when the whole line is bold, as in `**Title**`.
pub(crate) fn bold_title(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix("**")?.strip_suffix("**")?.trim();
    (!inner.is_empty() && !inner.contains("**")).then_some(inner)
}

/// The status of a conversation's running turn that a client which attaches while the
/// turn runs must see at once, not only at the next draft: the newest `context` draft,
/// and the `compacting` draft while a compaction runs. It also holds the parts of the
/// current model call ([`LiveTurn`]).
#[derive(Debug, Default)]
pub(crate) struct LiveStatus {
    held: Mutex<Held>,
    turn: Mutex<Option<TurnParts>>,
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

    /// The turn ended, or a compaction did: `compacting` only, or the whole status with
    /// the parts of the turn.
    pub(crate) fn clear(&self, compacting_only: bool) {
        {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            held.compacting = None;
            if !compacting_only {
                held.context = None;
            }
        }
        // NOTE: never with the other lock held, so the two locks have no order.
        if !compacting_only {
            *self.parts() = None;
        }
    }

    /// The parts of the running turn's current model call, for the drafter to write.
    /// The guard is held for one short change, never across an await.
    pub(crate) fn parts(&self) -> MutexGuard<'_, Option<TurnParts>> {
        self.turn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the current model call of the running turn streamed so far; `None` when no
    /// turn runs or its first model call has not started.
    pub(crate) fn live_turn(&self) -> Option<LiveTurn> {
        self.parts().as_ref().map(TurnParts::live)
    }

    /// The drafts of the status, in the order a client must apply them.
    pub(crate) fn drafts(&self) -> Vec<ConversationDraft> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.context.iter().chain(held.compacting.iter()).cloned().collect()
    }
}
