//! Quarantine questions: turns parked until the user says whether to keep git settings
//! that a contained call changed.
//!
//! The question is not an approval of a tool call, because the call already ended. So it
//! has its own id ([`QuestionId`]) and its own table here. The actor takes a question out
//! before it records the answer, so a turn that expires or is interrupted at the same
//! moment finds nothing to withdraw and waits for that answer instead: exactly one
//! `surface_question_answered` is recorded per question.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_protocol::{QuestionId, TurnId};
use tokio::sync::oneshot;

/// The questions of one conversation that wait for an answer. Cheap to clone; clones
/// share the questions.
#[derive(Debug, Clone, Default)]
pub(crate) struct Questions {
    parked: Arc<Mutex<HashMap<QuestionId, Parked>>>,
}

#[derive(Debug)]
struct Parked {
    turn_id: TurnId,
    answer: oneshot::Sender<bool>,
}

/// A question that the actor took out to answer it.
#[derive(Debug)]
pub(crate) struct Taken {
    /// The turn that waits.
    pub(crate) turn_id: TurnId,
    answer: oneshot::Sender<bool>,
}

impl Taken {
    /// Hands `keep` to the waiting turn. False when the turn stopped waiting.
    pub(crate) fn answer(self, keep: bool) -> bool {
        self.answer.send(keep).is_ok()
    }
}

impl Questions {
    /// Parks `question_id` of `turn_id`; the receiver gets the user's answer, true to keep
    /// the changes.
    pub(crate) fn park(&self, turn_id: TurnId, question_id: QuestionId) -> oneshot::Receiver<bool> {
        let (answer, receiver) = oneshot::channel();
        self.lock().insert(question_id, Parked { turn_id, answer });
        receiver
    }

    /// Takes `question_id` out to answer it, if a turn waits for it.
    pub(crate) fn take(&self, question_id: QuestionId) -> Option<Taken> {
        self.lock()
            .remove(&question_id)
            .map(|parked| Taken { turn_id: parked.turn_id, answer: parked.answer })
    }

    /// Takes `question_id` out without an answer. True when it was parked; false when
    /// the actor took it to answer it.
    pub(crate) fn withdraw(&self, question_id: QuestionId) -> bool {
        self.lock().remove(&question_id).is_some()
    }

    /// Every parked question, for the actor's state.
    pub(crate) fn parked(&self) -> Vec<QuestionId> {
        let mut questions: Vec<QuestionId> = self.lock().keys().copied().collect();
        questions.sort_unstable();
        questions
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<QuestionId, Parked>> {
        // Nothing panics while it holds the lock, so a poisoned lock is still usable.
        self.parked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
