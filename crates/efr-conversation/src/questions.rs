//! Questions that park a turn until a person answers them: the quarantine question
//! (whether to keep git settings that a contained call changed, an answer of `bool`)
//! and the model's own question (the tool `ask_user`, an answer of
//! [`QuestionReply`](efr_protocol::QuestionReply)).
//!
//! Neither is an approval of a tool call, so each has its own id ([`QuestionId`]) and
//! its own table here. The actor takes a question out before it records the answer, so
//! a turn that expires or is interrupted at the same moment finds nothing to withdraw
//! and waits for that answer instead: exactly one answer event is recorded per
//! question.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_protocol::{QuestionId, TurnId};
use tokio::sync::oneshot;

/// The questions of one conversation that wait for an answer of type `A`. Each one
/// keeps a fact `F` that the actor checks an answer against before it takes the
/// question, such as the number of options. Cheap to clone; clones share the
/// questions.
#[derive(Debug)]
pub(crate) struct Questions<A, F = ()> {
    parked: Arc<Mutex<HashMap<QuestionId, Parked<A, F>>>>,
}

impl<A, F> Clone for Questions<A, F> {
    fn clone(&self) -> Self {
        Questions { parked: Arc::clone(&self.parked) }
    }
}

impl<A, F> Default for Questions<A, F> {
    fn default() -> Self {
        Questions { parked: Arc::default() }
    }
}

#[derive(Debug)]
struct Parked<A, F> {
    turn_id: TurnId,
    fact: F,
    answer: oneshot::Sender<A>,
}

/// A question that the actor took out to answer it.
#[derive(Debug)]
pub(crate) struct Taken<A> {
    /// The turn that waits.
    pub(crate) turn_id: TurnId,
    answer: oneshot::Sender<A>,
}

impl<A> Taken<A> {
    /// Hands `answer` to the waiting turn. False when the turn stopped waiting.
    pub(crate) fn answer(self, answer: A) -> bool {
        self.answer.send(answer).is_ok()
    }
}

impl<A, F: Copy> Questions<A, F> {
    /// Parks `question_id` of `turn_id` with its `fact`; the receiver gets the
    /// person's answer.
    pub(crate) fn park(
        &self,
        turn_id: TurnId,
        question_id: QuestionId,
        fact: F,
    ) -> oneshot::Receiver<A> {
        let (answer, receiver) = oneshot::channel();
        self.lock().insert(question_id, Parked { turn_id, fact, answer });
        receiver
    }

    /// The fact of `question_id` while a turn waits for it.
    pub(crate) fn fact(&self, question_id: QuestionId) -> Option<F> {
        self.lock().get(&question_id).map(|parked| parked.fact)
    }

    /// Takes `question_id` out to answer it, if a turn waits for it.
    pub(crate) fn take(&self, question_id: QuestionId) -> Option<Taken<A>> {
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

    fn lock(&self) -> MutexGuard<'_, HashMap<QuestionId, Parked<A, F>>> {
        // Nothing panics while it holds the lock, so a poisoned lock is still usable.
        self.parked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
