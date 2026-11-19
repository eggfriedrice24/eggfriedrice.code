//! The open tickets: the pushes that the Expo push service took and whose receipt
//! efrd did not read yet.
//!
//! Expo says to read a receipt 15 minutes after the send, and keeps it for 24 hours.
//! A ticket waits [`RECEIPT_DELAY`] after its send; one with no receipt in the answer
//! waits once more and then goes. At most [`MAX_RECEIPT_IDS`] wait (one receipts
//! request, [`ExpoPush::MAX_RECEIPT_IDS`]), the oldest goes
//! first, and a restart of efrd drops them all. The tickets stay in the order of their
//! due time, because each one waits the same delay from the moment it is added.

use std::collections::VecDeque;
use std::time::Duration;

use efr_http::{ExpoPush, TicketId};
use efr_protocol::{DeviceId, SecretText};
use jiff::Timestamp;

/// The most tickets that wait.
const MAX_RECEIPT_IDS: usize = ExpoPush::MAX_RECEIPT_IDS;

/// How long a ticket waits before efrd reads its receipt.
pub(crate) const RECEIPT_DELAY: Duration = Duration::from_secs(15 * 60);

/// One ticket that waits for its receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenTicket {
    pub(crate) id: TicketId,
    pub(crate) device: DeviceId,
    /// The token of the push, so a dead token goes only while the device still has it.
    pub(crate) token: SecretText,
    pub(crate) due: Timestamp,
    /// True after one read that had no receipt for it.
    pub(crate) asked: bool,
}

/// The open tickets, oldest first.
#[derive(Debug, Default)]
pub(crate) struct OpenTickets {
    open: VecDeque<OpenTicket>,
}

impl OpenTickets {
    /// Adds the ticket `id` of a push to `device` with `token`, sent at `now`. When
    /// [`MAX_RECEIPT_IDS`] wait already, the oldest goes.
    pub(crate) fn add(
        &mut self,
        id: TicketId,
        device: DeviceId,
        token: SecretText,
        now: Timestamp,
    ) {
        let due = now.checked_add(RECEIPT_DELAY).unwrap_or(now);
        self.push(OpenTicket { id, device, token, due, asked: false });
    }

    /// Adds `ticket` again after a read at `now` that had no receipt for it, once; a
    /// ticket that was asked before goes.
    pub(crate) fn again(&mut self, ticket: OpenTicket, now: Timestamp) {
        if ticket.asked {
            return;
        }
        let due = now.checked_add(RECEIPT_DELAY).unwrap_or(now);
        self.push(OpenTicket { due, asked: true, ..ticket });
    }

    /// When the oldest ticket is due; `None` while no ticket waits, so no timer runs.
    pub(crate) fn next_due(&self) -> Option<Timestamp> {
        self.open.front().map(|ticket| ticket.due)
    }

    /// Takes the tickets that are due at `now`, oldest first.
    pub(crate) fn take_due(&mut self, now: Timestamp) -> Vec<OpenTicket> {
        let due = self.open.iter().take_while(|ticket| ticket.due <= now).count();
        self.open.drain(..due).collect()
    }

    fn push(&mut self, ticket: OpenTicket) {
        if self.open.len() >= MAX_RECEIPT_IDS {
            self.open.pop_front();
        }
        self.open.push_back(ticket);
    }
}

#[cfg(test)]
mod tests;
