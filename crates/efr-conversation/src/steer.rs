//! Steering: guidance the user adds to the running turn.
//!
//! The actor takes a [`Reservation`], records `turn_steered` with the command's receipt
//! and then pushes the text with the event's sequence number through the reservation.
//! The turn takes everything waiting before each model call, records
//! `steering_delivered` with those numbers and sends the texts as user messages, so the
//! model reads them at the next step; a turn that would end with steering still waiting
//! makes one more model call instead, so no guidance goes unanswered.
//!
//! A turn that ends closes its steering. When the model answers without a tool call,
//! the turn waits for the open reservations and closes only when nothing waits. After
//! that, [`Steering::reserve`] gives nothing and the actor treats the steer as late, so
//! the user never gets an answer for guidance that no model call will read.
//!
//! An interrupt can take back the steers that wait ([`Steering::take_unread`]): the
//! actor sends them again as a new prompt. The turn and the actor take under the same
//! lock, so each steer goes to exactly one of them.

use std::collections::VecDeque;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_protocol::Seq;
use tokio::sync::Notify;

/// One steer that the actor recorded: the sequence number of its `turn_steered` event
/// and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Steer {
    pub(crate) seq: Seq,
    pub(crate) text: String,
}

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
    /// Recorded steers that no model call took yet, oldest first.
    waiting: VecDeque<Steer>,
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

    /// Takes every waiting steer, oldest first, for the next model call.
    pub(crate) fn take(&self) -> Vec<Steer> {
        self.lock().waiting.drain(..).collect()
    }

    /// Takes the waiting steers whose sequence number is in `seqs`, oldest first, so
    /// no model call reads them. The others keep waiting.
    pub(crate) fn take_unread(&self, seqs: &[Seq]) -> Vec<Steer> {
        if seqs.is_empty() {
            return Vec::new();
        }
        let mut state = self.lock();
        let (taken, kept): (VecDeque<Steer>, VecDeque<Steer>) =
            state.waiting.drain(..).partition(|steer| seqs.contains(&steer.seq));
        state.waiting = kept;
        taken.into()
    }

    /// Puts `steers` that [`take_unread`](Self::take_unread) took back in front of the
    /// waiting ones, when the actor could not record what it took them for.
    pub(crate) fn put_back(&self, steers: Vec<Steer>) {
        let mut state = self.lock();
        let mut waiting: VecDeque<Steer> = steers.into();
        waiting.append(&mut state.waiting);
        waiting.make_contiguous().sort_by_key(|steer| steer.seq);
        state.waiting = waiting;
    }

    /// True when guidance waits.
    pub(crate) fn is_waiting(&self) -> bool {
        !self.lock().waiting.is_empty()
    }

    /// Closes the steering when nothing waits, and returns true. Waits first until
    /// every open reservation is pushed or dropped, so a text that the actor records
    /// now is either read by the turn or treated as late.
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

    fn release(&self, steer: Option<Steer>) {
        {
            let mut state = self.lock();
            state.reserved = state.reserved.saturating_sub(1);
            if let Some(steer) = steer {
                state.waiting.push_back(steer);
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
    /// Adds the text of the `turn_steered` event `seq` for the next model call.
    pub(crate) fn push(mut self, seq: Seq, text: String) {
        self.open = false;
        self.steering.release(Some(Steer { seq, text }));
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
