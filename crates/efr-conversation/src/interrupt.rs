//! The two-phase interrupt.
//!
//! Phase one belongs to the actor: it records `turn_interrupt_requested` with the
//! command's receipt and raises the turn's [`Interrupt`]. Phase two belongs to the turn:
//! at its next await point that can be interrupted (the provider's stream, a parked
//! approval, a running tool call) it drops what it was waiting on, records what was
//! streamed so far, and only then records `turn_interrupted`. A turn that finished on
//! its own between the two phases keeps the end it reached.

use tokio::sync::watch;

/// The interrupt of one turn. Cheap to clone; clones share it.
#[derive(Debug, Clone)]
pub(crate) struct Interrupt {
    raised: watch::Sender<bool>,
}

impl Interrupt {
    pub(crate) fn new() -> Self {
        Interrupt { raised: watch::channel(false).0 }
    }

    /// Phase one: asks the turn to stop. Raising it twice is the same as once.
    pub(crate) fn raise(&self) {
        self.raised.send_replace(true);
    }

    /// True once [`raise`](Self::raise) was called.
    pub(crate) fn is_raised(&self) -> bool {
        *self.raised.borrow()
    }

    /// Completes once the interrupt is raised; at once when it already is.
    pub(crate) async fn raised(&self) {
        let mut receiver = self.raised.subscribe();
        // The sender lives in `self`, so `wait_for` fails only after `self` is gone,
        // which cannot happen while this borrows it.
        let _ = receiver.wait_for(|raised| *raised).await;
    }
}

#[cfg(test)]
mod tests;
