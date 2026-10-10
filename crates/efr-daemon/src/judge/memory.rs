//! What the judge keeps between exits (9.5): the verdicts of each conversation's
//! current turn, the models that each conversation skips, and the models that refused
//! the output form.
//!
//! One lock guards it, and no await ever holds it. The verdicts of a turn go when the
//! next turn of the conversation asks, so a conversation keeps at most one turn's
//! verdicts. efrd tells the judge nothing when a conversation closes, so the memory
//! keeps at most [`MAX_CONVERSATIONS`] conversations and drops the one that asked
//! least recently.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use efr_protocol::{ChainStep, ConversationId, Judgement, TurnId};

/// The most conversations whose verdicts and skipped models the judge keeps.
pub(crate) const MAX_CONVERSATIONS: usize = 256;

/// A verdict that a later exit of the same turn reads again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cached {
    pub(crate) judgement: Judgement,
    pub(crate) model: String,
    pub(crate) chain_step: ChainStep,
}

/// The judge's memory.
#[derive(Debug, Default)]
pub(crate) struct Memory {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    conversations: HashMap<ConversationId, Kept>,
    /// The models that refused the output form, for the daemon's life.
    no_format: HashSet<String>,
    /// A counter of the asks, which says which conversation asked least recently.
    asks: u64,
}

/// What the judge keeps for one conversation.
#[derive(Debug, Default)]
struct Kept {
    /// The turn whose verdicts [`verdicts`](Self::verdicts) holds.
    turn: Option<TurnId>,
    /// The verdicts of that turn, by the hash of the record without its counters.
    verdicts: HashMap<String, Cached>,
    /// The models that the chain skips for the rest of the conversation, with the
    /// reason.
    skipped: BTreeMap<String, &'static str>,
    /// The ask of [`Inner::asks`] when the conversation last asked.
    asked: u64,
}

impl Memory {
    /// The verdict of `turn` for the record whose key is `key`, when the turn judged it
    /// before. A new turn of the conversation drops the verdicts of the turn before.
    pub(crate) fn verdict(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        key: &str,
    ) -> Option<Cached> {
        let mut inner = self.lock();
        let kept = inner.kept(conversation);
        if kept.turn != Some(turn) {
            kept.turn = Some(turn);
            kept.verdicts.clear();
        }
        kept.verdicts.get(key).cloned()
    }

    /// Keeps `cached` as the verdict of `turn` for the record whose key is `key`.
    pub(crate) fn keep_verdict(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        key: String,
        cached: Cached,
    ) {
        let mut inner = self.lock();
        let kept = inner.kept(conversation);
        if kept.turn != Some(turn) {
            kept.turn = Some(turn);
            kept.verdicts.clear();
        }
        kept.verdicts.insert(key, cached);
    }

    /// Why the conversation skips `model`, when it does.
    pub(crate) fn skipped(
        &self,
        conversation: ConversationId,
        model: &str,
    ) -> Option<&'static str> {
        let inner = self.lock();
        inner.conversations.get(&conversation).and_then(|kept| kept.skipped.get(model).copied())
    }

    /// Skips `model` for the rest of the conversation, for `reason`.
    pub(crate) fn skip(&self, conversation: ConversationId, model: &str, reason: &'static str) {
        self.lock().kept(conversation).skipped.insert(model.to_owned(), reason);
    }

    /// False when `model` refused the output form before.
    pub(crate) fn takes_format(&self, model: &str) -> bool {
        !self.lock().no_format.contains(model)
    }

    /// Remembers for the daemon's life that `model` refused the output form.
    pub(crate) fn refuses_format(&self, model: &str) {
        self.lock().no_format.insert(model.to_owned());
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Nothing panics while it holds the lock, so a poisoned lock is still usable.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Inner {
    /// The memory of `conversation`, made when it has none, after the conversation that
    /// asked least recently goes when the memory is full.
    fn kept(&mut self, conversation: ConversationId) -> &mut Kept {
        self.asks += 1;
        let asks = self.asks;
        if !self.conversations.contains_key(&conversation)
            && self.conversations.len() >= MAX_CONVERSATIONS
            && let Some(oldest) =
                self.conversations.iter().min_by_key(|(_, kept)| kept.asked).map(|(id, _)| *id)
        {
            self.conversations.remove(&oldest);
        }
        let kept = self.conversations.entry(conversation).or_default();
        kept.asked = asks;
        kept
    }
}

#[cfg(test)]
mod tests;
