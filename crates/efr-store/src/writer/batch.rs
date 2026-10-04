//! What one commit writes, and what it committed.

use std::sync::Arc;

use efr_protocol::{ConversationId, Event, EventEnvelope, Seq};

/// Everything that one transaction writes: events, in order. The projections follow
/// from the events and are written in the same transaction.
#[derive(Debug, Clone, Default, PartialEq)]
#[must_use]
pub struct Batch {
    pub(crate) events: Vec<(Option<ConversationId>, Event)>,
}

impl Batch {
    /// An empty batch.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an event of a conversation.
    pub fn event(mut self, conversation_id: ConversationId, event: Event) -> Self {
        self.events.push((Some(conversation_id), event));
        self
    }

    /// Adds a daemon-wide event that belongs to no conversation, such as
    /// `login_completed`.
    pub fn global_event(mut self, event: Event) -> Self {
        self.events.push((None, event));
        self
    }

    /// True when the batch writes nothing.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The result of a committed batch, which is also what subscribers receive. Cloning
/// is cheap: the events are shared.
#[derive(Debug, Clone, PartialEq)]
pub struct Committed {
    events: Arc<[EventEnvelope]>,
    last_seq: Seq,
}

impl Committed {
    pub(crate) fn new(events: Vec<EventEnvelope>, last_seq: Seq) -> Self {
        Committed { events: events.into(), last_seq }
    }

    /// The committed events with their sequence numbers and time, in order.
    pub fn events(&self) -> &[EventEnvelope] {
        &self.events
    }

    /// The sequence number of the first committed event, or `None` when the batch had
    /// no events.
    pub fn first_seq(&self) -> Option<Seq> {
        self.events.first().map(|envelope| envelope.seq)
    }

    /// The high-water mark after the commit: the sequence number of the newest event
    /// in the log, which is the batch's last event when it had any.
    pub fn last_seq(&self) -> Seq {
        self.last_seq
    }
}
