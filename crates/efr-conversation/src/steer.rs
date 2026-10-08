//! Steering: guidance the user adds to the running turn.
//!
//! The actor takes a [`Reservation`], records `turn_steered` with the command's receipt
//! and then pushes the text through the reservation. The turn takes everything waiting
//! before each model call and sends it as user messages, so the model reads it at the
//! next step; a turn that would end with steering still waiting makes one more model
//! call instead, so no guidance goes unanswered.
//!
//! A turn that ends closes its steering. When the model answers without a tool call,
//! the turn waits for the open reservations and closes only when nothing waits. After
//! that, [`Steering::reserve`] gives nothing and the actor refuses the steer, so the
//! user never gets an answer for guidance that no model call will read.

use std::collections::VecDeque;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

/// The steering of one turn. Cheap to clone; clones share it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Steering {
    shared: Arc<Shared>,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    /// Wakes a turn that waits for the open reservations.
    settled: Notify,
}

#[derive(Debug, Default)]
struct State {
    waiting: VecDeque<String>,
    /// Reservations that are not pushed or dropped yet.
    reserved: usize,
    /// Set when the turn reads no more steering.
    closed: bool,
}

/// The place of one steering text that the actor records before it pushes the text.
/// Dropping it without [`push`](Self::push) gives the place back.
#[derive(Debug)]
pub(crate) struct Reservation {
    steering: Steering,
    open: bool,
}

impl Steering {
    /// A place for one text, or `None` when the turn reads no more steering.
    pub(crate) fn reserve(&self) -> Option<Reservation> {
        let mut state = self.lock();
        if state.closed {
            return None;
        }
        state.reserved += 1;
        Some(Reservation { steering: self.clone(), open: true })
    }

    /// Takes every waiting text, oldest first.
    pub(crate) fn take(&self) -> Vec<String> {
        self.lock().waiting.drain(..).collect()
    }

    /// True when guidance waits.
    pub(crate) fn is_waiting(&self) -> bool {
        !self.lock().waiting.is_empty()
    }

    /// Closes the steering when nothing waits, and returns true. Waits first until
    /// every open reservation is pushed or dropped, so a text that the actor records
    /// now is either read by the turn or refused.
    pub(crate) async fn close_if_idle(&self) -> bool {
        loop {
            let mut settled = pin!(self.shared.settled.notified());
            // NOTE: enabled before the check, so a release between the check and the
            // await still wakes this turn.
            settled.as_mut().enable();
            {
                let mut state = self.lock();
                if state.reserved == 0 {
                    if !state.waiting.is_empty() {
                        return false;
                    }
                    state.closed = true;
                    return true;
                }
            }
            settled.await;
        }
    }

    /// Closes the steering whatever waits, for a turn that ends another way.
    pub(crate) fn close(&self) {
        self.lock().closed = true;
    }

    fn release(&self, text: Option<String>) {
        {
            let mut state = self.lock();
            state.reserved = state.reserved.saturating_sub(1);
            if let Some(text) = text {
                state.waiting.push_back(text);
            }
        }
        self.shared.settled.notify_waiters();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Nothing panics while it holds the lock, so a poisoned lock is still usable.
        self.shared.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Reservation {
    /// Adds the text for the next model call.
    pub(crate) fn push(mut self, text: String) {
        self.open = false;
        self.steering.release(Some(text));
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.open {
            self.steering.release(None);
        }
    }
}

#[cfg(test)]
mod tests;
