//! The pushes that went out, so one question pushes once: the notices follower and a
//! waiting question can both hand it over, and a question can wait again later.

use std::collections::{HashSet, VecDeque};

use efr_protocol::{CallId, ConversationId, Seq, TurnId};

/// The most pushes that are remembered; the oldest goes first. A question that waits
/// longer than this many other pushes can push again.
const MAX_REMEMBERED: usize = 1024;

/// What a push is about in its conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Subject {
    /// A call that waits: an approval, a question of the model, a command that waits
    /// for input.
    Call(CallId),
    /// The end of a turn.
    Turn(TurnId),
    /// Another event, by its sequence number.
    Event(Seq),
}

/// The newest pushes, by conversation and subject.
#[derive(Debug, Default)]
pub(crate) struct Repeats {
    order: VecDeque<(ConversationId, Subject)>,
    seen: HashSet<(ConversationId, Subject)>,
}

impl Repeats {
    /// True, and remembers it, when no push about `subject` of `conversation` went
    /// out before; false for a repeat.
    pub(crate) fn first(&mut self, conversation: ConversationId, subject: Subject) -> bool {
        let key = (conversation, subject);
        if !self.seen.insert(key) {
            return false;
        }
        if self.order.len() >= MAX_REMEMBERED
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        self.order.push_back(key);
        true
    }
}

#[cfg(test)]
mod tests;
