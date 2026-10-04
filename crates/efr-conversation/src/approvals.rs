//! Approvals: tool calls parked until the user answers.
//!
//! When the engine says Ask, the turn parks the call here before it records
//! `approval_requested`, so an answer that arrives right after the event commits finds
//! it. The actor answers in two steps: it records `approval_resolved` with the
//! command's receipt (the store refuses an answer to a call that is not pending, so of
//! two racing answers only one commits) and then hands the decision to the parked turn.
//! A parked call that can no longer be answered (the turn was interrupted, the wait
//! timed out, the actor stopped) is taken out and recorded as `approval_expired`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_permissions::Decision;
use efr_protocol::{ApprovalDecision, CallId, TurnId};
use tokio::sync::oneshot;

/// The calls of one conversation that wait for an answer. Cheap to clone; clones share
/// the calls.
#[derive(Debug, Clone, Default)]
pub(crate) struct Approvals {
    parked: Arc<Mutex<HashMap<CallId, Parked>>>,
}

#[derive(Debug)]
struct Parked {
    turn_id: TurnId,
    answer: oneshot::Sender<ApprovalDecision>,
}

impl Approvals {
    /// Parks `call_id` of `turn_id`; the receiver gets the user's decision.
    pub(crate) fn park(
        &self,
        turn_id: TurnId,
        call_id: CallId,
    ) -> oneshot::Receiver<ApprovalDecision> {
        let (answer, receiver) = oneshot::channel();
        self.lock().insert(call_id, Parked { turn_id, answer });
        receiver
    }

    /// The turn that waits for an answer about `call_id`, if one does.
    pub(crate) fn waiting_turn(&self, call_id: CallId) -> Option<TurnId> {
        self.lock().get(&call_id).map(|parked| parked.turn_id)
    }

    /// Hands `decision` to the turn parked on `call_id`. False when no turn waits, or
    /// when it stopped waiting.
    pub(crate) fn answer(&self, call_id: CallId, decision: ApprovalDecision) -> bool {
        let parked = self.lock().remove(&call_id);
        parked.is_some_and(|parked| parked.answer.send(decision).is_ok())
    }

    /// Takes `call_id` out without an answer. True when it was parked.
    pub(crate) fn withdraw(&self, call_id: CallId) -> bool {
        self.lock().remove(&call_id).is_some()
    }

    /// Takes out every call of `turn_id`, for a turn that stopped without withdrawing
    /// them.
    pub(crate) fn withdraw_turn(&self, turn_id: TurnId) -> Vec<CallId> {
        let mut parked = self.lock();
        let mut calls: Vec<CallId> = parked
            .iter()
            .filter(|(_, p)| p.turn_id == turn_id)
            .map(|(call_id, _)| *call_id)
            .collect();
        calls.sort_unstable();
        for call_id in &calls {
            parked.remove(call_id);
        }
        calls
    }

    /// Every parked call, for the actor's state.
    pub(crate) fn parked(&self) -> Vec<CallId> {
        let mut calls: Vec<CallId> = self.lock().keys().copied().collect();
        calls.sort_unstable();
        calls
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<CallId, Parked>> {
        // Nothing panics while it holds the lock, so a poisoned lock is still usable.
        self.parked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The one line an approval request shows: the tool and what needs approval, such as
/// `write_file: write /home/u/.zshrc (user config)`.
pub(crate) fn summary(tool: &str, decision: &Decision) -> String {
    let subjects: Vec<String> =
        decision.deciding().map(|reason| reason.subject.to_string()).collect();
    if subjects.is_empty() { tool.to_owned() } else { format!("{tool}: {}", subjects.join("; ")) }
}

/// What the model reads when the engine refuses a call: every reason that refused it,
/// each naming the path and its class.
pub(crate) fn denial(tool: &str, decision: &Decision) -> String {
    let reasons: Vec<String> = decision.deciding().map(ToString::to_string).collect();
    format!(
        "Permission denied for the {tool} call: {}. Do not try to reach the same target \
         another way; tell the user what you needed instead.",
        reasons.join("; ")
    )
}

#[cfg(test)]
mod tests;
