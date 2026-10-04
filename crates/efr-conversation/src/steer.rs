//! Steering: guidance the user adds to the running turn.
//!
//! The actor records `turn_steered` with the command's receipt and pushes the text
//! here. The turn takes everything waiting before each model call and sends it as user
//! messages, so the model reads it at the next step; a turn that would end with steering
//! still waiting makes one more model call instead, so no guidance goes unanswered.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The steering waiting for the running turn. Cheap to clone; clones share it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Steering {
    waiting: Arc<Mutex<VecDeque<String>>>,
}

impl Steering {
    /// Adds guidance for the next model call.
    pub(crate) fn push(&self, text: String) {
        self.lock().push_back(text);
    }

    /// Takes every waiting text, oldest first.
    pub(crate) fn take(&self) -> Vec<String> {
        self.lock().drain(..).collect()
    }

    /// True when guidance waits.
    pub(crate) fn is_waiting(&self) -> bool {
        !self.lock().is_empty()
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<String>> {
        // Nothing panics while it holds the lock, so a poisoned lock is still usable.
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
