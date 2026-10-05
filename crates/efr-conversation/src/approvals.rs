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

use efr_permissions::{Decision, Effect, Subject};
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

/// The most words after the program that a named part of a line shows.
const PART_WORDS: usize = 3;

/// The longest word after the program that a named part of a line shows whole.
const PART_WORD_CHARS: usize = 24;

/// What a named part of a line shows in place of the words it leaves out.
const LEFT_OUT: &str = "...";

/// What an approval request shows: the tool and what needs approval, such as
/// `write_file: write /home/u/.zshrc (user config)`. A command line comes first even
/// when only a path it reads needs approval, as for `rg TOKEN ~`, because the user
/// approves the whole line. When some simple commands of a line of several ask, a
/// second line names them, such as `asks for: hostnamectl, systemctl --failed`, so the
/// user need not look for them in a long line.
pub(crate) fn summary(tool: &str, decision: &Decision) -> String {
    let mut subjects: Vec<String> =
        decision.deciding().map(|reason| reason.subject.to_string()).collect();
    let command = decision.reasons().iter().find(|reason| {
        matches!(reason.subject, Subject::Command { .. }) && reason.effect != decision.effect()
    });
    if let Some(command) = command {
        subjects.insert(0, command.subject.to_string());
    }
    let mut text = if subjects.is_empty() {
        tool.to_owned()
    } else {
        format!("{tool}: {}", subjects.join("; "))
    };
    // NOTE: the `asks for` line below must be the summary's only line break, so a path
    // whose name holds one cannot pass for it.
    text = text.replace('\n', "\\n").replace('\r', "\\r");
    let asking = asking_parts(decision);
    if !asking.is_empty() {
        text.push_str("\nasks for: ");
        text.push_str(&asking.join(", "));
    }
    text
}

/// The simple commands of the call's line that ask, each named by [`part_name`], once
/// each, in the order of the line.
fn asking_parts(decision: &Decision) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for reason in decision.deciding().filter(|reason| reason.effect == Effect::Ask) {
        let Subject::Command { deciding, .. } = &reason.subject else {
            continue;
        };
        for words in deciding {
            let name = part_name(words);
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// A short name of one simple command: the program and at most [`PART_WORDS`] words
/// after it, such as `systemctl --failed`.
///
/// NOTE: the name repeats as little of the line as it can, because an argument may be
/// a token or a password, and the approval goes to the event log, the notices and
/// other clients. A word ends the name when it is long or holds a character other than
/// letters, digits and `._/:@%+,-`, as a quoted header, an assignment or a URL with a
/// query does, and the value of `--option=value` never shows. The line itself still
/// shows above the name, because the user must see what they approve.
fn part_name(words: &[String]) -> String {
    let mut words = words.iter();
    let Some(program) = words.next().filter(|program| program.chars().all(plain)) else {
        return LEFT_OUT.to_owned();
    };
    let mut name = vec![program.clone()];
    for (shown, word) in words.enumerate() {
        if shown == PART_WORDS {
            name.push(LEFT_OUT.to_owned());
            break;
        }
        if word.starts_with('-')
            && let Some((option, _)) = word.split_once('=')
            && option.chars().all(plain)
        {
            name.push(format!("{option}={LEFT_OUT}"));
            continue;
        }
        if word.is_empty() || word.chars().count() > PART_WORD_CHARS || !word.chars().all(plain) {
            name.push(LEFT_OUT.to_owned());
            break;
        }
        name.push(word.clone());
    }
    name.join(" ")
}

/// A character that a named part shows: a letter, a digit or one of `._/:@%+,-`.
fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | ':' | '@' | '%' | '+' | ',' | '-')
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
