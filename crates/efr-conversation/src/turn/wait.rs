//! The wait for a person: the one loop of every question that parks a turn, an
//! approval, the quarantine question and the model's question (`ask_user`).
//!
//! The question waits for its answer, the interrupt, or a timeout. Who is there is read
//! when the question parks and again every [`PRESENCE_CHECK`] while it waits. While
//! nobody is there, the question waits only `presence.away_wait_secs` from the moment
//! the turn sees it, and the question goes to the user's phones once; a user who comes
//! back gets the normal wait again: `conversation.approval_timeout_secs`, or no end. A
//! question about a guarded delete also has a deadline of its own. An answer wins during
//! the whole wait.

use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{CallId, Event, Presence, QuestionId};
use jiff::Timestamp;
use tokio::sync::oneshot;

use super::Turn;
use super::coalesce::sleep_or_pending;
use crate::ConversationError;
use crate::presence::PRESENCE_CHECK;
use crate::questions::Questions;

/// Who answers a question, which says when nobody is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Answerer {
    /// The user, at an approval or at the quarantine question: nobody is there while
    /// the user is away, or in a turn that a job notice started while no client that
    /// answers input follows the conversation. Nobody typed the input of such a turn,
    /// so a user who typed in another terminal a while ago does not count as there
    /// (phase C).
    User,
    /// A client that can answer the model's question: nobody is there while the user is
    /// away, or while no such client follows the conversation, in every turn.
    Questions,
}

/// One wait for a person.
#[derive(Debug, Clone, Copy)]
pub(super) struct Wait {
    /// The tool call of the question, which the push to the phones names.
    pub(super) call_id: CallId,
    pub(super) answerer: Answerer,
    /// The longest wait of a question about a guarded delete; `None` for every other
    /// question.
    pub(super) deadline: Option<Duration>,
    /// True for a question with a deadline that nobody is there to answer: it times
    /// out at once and goes to no phone.
    pub(super) at_once: bool,
}

/// How a wait for a person ended.
#[derive(Debug)]
pub(super) enum Waited<T> {
    /// The actor handed over an answer; `None` when it took the question but could
    /// not record the answer.
    Answer(Option<T>),
    /// The user interrupted the turn.
    Interrupted,
    /// A timeout passed: the away wait, the approval timeout or the deadline.
    TimedOut,
}

/// A wait that ended, with what the caller records about it.
#[derive(Debug)]
pub(super) struct Ended<T> {
    pub(super) waited: Waited<T>,
    /// True when nobody was there when the wait ended.
    pub(super) away: bool,
    /// When the question parked.
    pub(super) parked: Timestamp,
}

impl Turn {
    /// True when nobody is there to answer a question of `answerer` now.
    pub(super) fn nobody_there(&self, answerer: Answerer) -> bool {
        let presence = &self.shared.deps.presence;
        let conversation_id = self.shared.conversation_id;
        if presence.presence(conversation_id) == Presence::Away {
            return true;
        }
        match answerer {
            Answerer::User => self.spec.is_wake() && !presence.followed(conversation_id),
            Answerer::Questions => !presence.question_followed(conversation_id),
        }
    }

    /// Waits until a person answers through `answer`, the turn is interrupted, or a
    /// timeout of `wait` passes. The timeouts are read from the latest settings, so a
    /// reload reaches the next question of a running turn.
    pub(super) async fn wait_for_person<T>(
        &self,
        wait: Wait,
        answer: &mut oneshot::Receiver<T>,
    ) -> Ended<T> {
        let config = self.shared.config.current();
        let interrupt = self.control.interrupt.clone();
        let clock = Arc::clone(&self.shared.deps.clock);
        let parked = clock.now();
        // NOTE: presence is read when the question parks and again every
        // `PRESENCE_CHECK` while it waits, so a user who leaves, or says so with
        // `efr away`, after it parked still gets the away wait, from the moment efr
        // sees it. A user who comes back gets the normal wait again.
        let mut away_from: Option<Duration> = None;
        let waited = loop {
            let elapsed =
                Duration::try_from(clock.now().duration_since(parked)).unwrap_or_default();
            if self.nobody_there(wait.answerer) {
                away_from.get_or_insert(elapsed);
                // NOTE: at each look, because the reader can refuse a push now (you
                // are still at the terminal) and send it at a later look.
                if !wait.at_once {
                    self.shared.deps.presence.push(self.shared.conversation_id, wait.call_id);
                }
            } else {
                away_from = None;
            }
            let left = match away_from {
                Some(from) => Some((from + config.away_wait).saturating_sub(elapsed)),
                None => config.approval_timeout.map(|timeout| timeout.saturating_sub(elapsed)),
            };
            // NOTE: the smallest of the deadline of a guarded delete and the other waits.
            let left = match (left, wait.deadline) {
                _ if wait.at_once => Some(Duration::ZERO),
                (Some(left), Some(deadline)) => Some(left.min(deadline.saturating_sub(elapsed))),
                (None, Some(deadline)) => Some(deadline.saturating_sub(elapsed)),
                (left, None) => left,
            };
            if wait.deadline.is_some() && left == Some(Duration::ZERO) {
                break Waited::TimedOut;
            }
            tokio::select! {
                biased;
                () = interrupt.raised() => break Waited::Interrupted,
                received = &mut *answer => break Waited::Answer(received.ok()),
                () = sleep_or_pending(&*clock, left) => break Waited::TimedOut,
                () = clock.sleep(PRESENCE_CHECK) => {}
            }
        };
        Ended { waited, away: away_from.is_some(), parked }
    }

    /// The answer to the question `question_id` of `questions` after its wait ended as
    /// `waited`, or `None` when nobody answered. Without an answer the turn takes the
    /// question back and records `unanswered`, the event of its end. When the actor
    /// took the question at this moment, the actor records the answer, so the turn
    /// waits for it through `answer` and records none of its own: each question gets
    /// exactly one end.
    pub(super) async fn settle<A, F: Copy>(
        &self,
        questions: &Questions<A, F>,
        question_id: QuestionId,
        waited: Waited<A>,
        answer: oneshot::Receiver<A>,
        unanswered: Event,
    ) -> Result<Option<A>, ConversationError> {
        match waited {
            // NOTE: `None` when the actor took the question but could not record the
            // answer.
            Waited::Answer(answer) => Ok(answer),
            Waited::Interrupted | Waited::TimedOut if questions.withdraw(question_id) => {
                self.record(vec![unanswered]).await?;
                Ok(None)
            }
            Waited::Interrupted | Waited::TimedOut => Ok(answer.await.ok()),
        }
    }
}

// NOTE: the tests of the model's question in `turn/tests/ask.rs` share its helpers.
#[cfg(test)]
pub(super) mod tests;
