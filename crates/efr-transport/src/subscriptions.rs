//! Bounded per-subscriber queues: a fan-out never waits for its slowest consumer.
//!
//! A producer (the event broadcast, a PTY's output) holds one [`SubscriptionSender`]
//! per subscriber and offers each item with its sequence number. An offer never waits:
//! when the subscriber's queue already holds [`SUBSCRIBER_QUEUE_FRAMES`] items, that
//! subscription is closed instead. The request that serves the subscriber drains its
//! [`SubscriptionReceiver`]: first every item that was queued before the overflow, then
//! [`Delivery::Overflowed`] with `last_seq`, the sequence number of the last item it
//! received. [`Responder::forward`](crate::Responder::forward) turns that into the
//! `overflow` error frame, and the client subscribes again after `last_seq` without a
//! gap.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use efr_protocol::Seq;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// How many items a subscriber may have queued before its subscription is closed.
pub const SUBSCRIBER_QUEUE_FRAMES: usize = 64;

/// A new subscription with a queue of [`SUBSCRIBER_QUEUE_FRAMES`] items.
pub fn subscription<T>() -> (SubscriptionSender<T>, SubscriptionReceiver<T>) {
    bounded(SUBSCRIBER_QUEUE_FRAMES)
}

fn bounded<T>(capacity: usize) -> (SubscriptionSender<T>, SubscriptionReceiver<T>) {
    let (tx, rx) = mpsc::channel(capacity);
    let overflowed = Arc::new(AtomicBool::new(false));
    (
        SubscriptionSender { tx: Some(tx), overflowed: Arc::clone(&overflowed) },
        SubscriptionReceiver { rx, overflowed, last_seq: None, done: false },
    )
}

/// What became of one offered item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a producer drops a subscriber whose offer did not queue"]
pub enum Offer {
    /// The item is queued for the subscriber.
    Queued,
    /// The queue was full: the item was dropped and the subscription is now closed. The
    /// subscriber receives what was queued before it and then the overflow.
    Overflowed,
    /// The subscription was already closed, by an earlier overflow or because the
    /// subscriber went away. The producer forgets this sender.
    Closed,
}

/// One step of a subscription, as its request reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery<T> {
    /// The next item.
    Item {
        /// The item's sequence number.
        seq: Seq,
        /// The item.
        item: T,
    },
    /// The subscriber fell behind and the subscription is closed. Nothing follows.
    Overflowed {
        /// The sequence number of the last item the subscriber received.
        last_seq: Seq,
    },
}

/// The producer's side of one subscription. Offers never wait.
pub struct SubscriptionSender<T> {
    tx: Option<mpsc::Sender<(Seq, T)>>,
    overflowed: Arc<AtomicBool>,
}

impl<T> SubscriptionSender<T> {
    /// Offers `item` with sequence number `seq` without waiting. Sequence numbers must
    /// grow from offer to offer.
    pub fn offer(&mut self, seq: Seq, item: T) -> Offer {
        let Some(tx) = &self.tx else {
            return Offer::Closed;
        };
        match tx.try_send((seq, item)) {
            Ok(()) => Offer::Queued,
            Err(TrySendError::Full(_)) => {
                // NOTE: the flag is stored before the sender is dropped, so the receiver,
                // which reads it after it sees the channel close, always finds it set.
                self.overflowed.store(true, Ordering::Release);
                self.tx = None;
                Offer::Overflowed
            }
            Err(TrySendError::Closed(_)) => {
                self.tx = None;
                Offer::Closed
            }
        }
    }

    /// True when offers can no longer queue: after an overflow, or when the subscriber
    /// went away.
    pub fn is_closed(&self) -> bool {
        self.tx.as_ref().is_none_or(mpsc::Sender::is_closed)
    }
}

impl<T> fmt::Debug for SubscriptionSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionSender").field("closed", &self.is_closed()).finish()
    }
}

/// The subscriber's side of one subscription.
pub struct SubscriptionReceiver<T> {
    rx: mpsc::Receiver<(Seq, T)>,
    overflowed: Arc<AtomicBool>,
    last_seq: Option<Seq>,
    done: bool,
}

impl<T> SubscriptionReceiver<T> {
    /// Marks every item up to and including `seq` as already delivered, by a replay or a
    /// snapshot that the request sent before it forwards live items. Queued items at or
    /// below it are skipped, and an overflow reports at least `seq`.
    pub fn skip_through(&mut self, seq: Seq) {
        self.last_seq = Some(self.last_seq.map_or(seq, |last| last.max(seq)));
    }

    /// The last sequence number delivered so far, or marked by
    /// [`skip_through`](Self::skip_through).
    pub fn last_seq(&self) -> Option<Seq> {
        self.last_seq
    }

    /// The next step of the subscription; `None` once the producer finished or after an
    /// overflow was delivered.
    pub async fn recv(&mut self) -> Option<Delivery<T>> {
        if self.done {
            return None;
        }
        loop {
            let Some((seq, item)) = self.rx.recv().await else {
                self.done = true;
                if self.overflowed.load(Ordering::Acquire) {
                    let last_seq = self.last_seq.unwrap_or(Seq::ZERO);
                    return Some(Delivery::Overflowed { last_seq });
                }
                return None;
            };
            if self.last_seq.is_some_and(|last| seq <= last) {
                continue;
            }
            self.last_seq = Some(seq);
            return Some(Delivery::Item { seq, item });
        }
    }
}

impl<T> fmt::Debug for SubscriptionReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionReceiver")
            .field("last_seq", &self.last_seq)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
