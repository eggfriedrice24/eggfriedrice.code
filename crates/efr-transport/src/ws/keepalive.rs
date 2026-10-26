//! The server's pings, which find a phone that went away without a close.
//!
//! Every [`PING_INTERVAL`] on the injected clock the keepalive sends a ping whose
//! payload is its number. A pong that names a ping answers it and every older one (RFC
//! 6455, section 5.5.3 lets a peer answer only the newest). When [`MISSED_PONGS`] pings
//! wait for their pong at a tick, the peer is dead: the keepalive records the close
//! reason and stops the reader, and the connection closes.
//!
//! A ping that the writer has no room for still counts as sent, so a peer that stops
//! reading is found the same way.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_stdx::time::Clock;
use fastwebsockets::{Frame, OpCode, Payload};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio_util::sync::CancellationToken;

use crate::WebSocketCloseReason;
use crate::ws::frames::CloseSlot;

/// How often the server pings.
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(25);

/// How many pings may wait for their pong at a tick before the peer counts as dead.
pub(crate) const MISSED_PONGS: u64 = 2;

/// The pings sent and the newest one answered.
#[derive(Debug, Default)]
pub(crate) struct Pings {
    state: Mutex<PingState>,
}

#[derive(Debug, Default)]
struct PingState {
    /// The number of the newest ping sent; pings count from 1.
    sent: u64,
    /// The number of the newest ping answered.
    answered: u64,
}

impl Pings {
    fn lock(&self) -> std::sync::MutexGuard<'_, PingState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts a new ping and returns its number.
    fn next(&self) -> u64 {
        let mut state = self.lock();
        state.sent += 1;
        state.sent
    }

    /// How many pings wait for their pong.
    fn outstanding(&self) -> u64 {
        let state = self.lock();
        state.sent - state.answered
    }

    /// Takes a pong with `payload`. A pong that names no ping of ours is allowed as a
    /// heartbeat (RFC 6455, section 5.5.3) and answers nothing.
    pub(crate) fn answer(&self, payload: &[u8]) {
        let Ok(number) = <[u8; 8]>::try_from(payload) else {
            return;
        };
        let number = u64::from_be_bytes(number);
        let mut state = self.lock();
        if number <= state.sent {
            state.answered = state.answered.max(number);
        }
    }
}

/// Pings the peer until it misses [`MISSED_PONGS`] pongs or the writer is gone. On a
/// timeout it records the close reason in `close` and cancels `dead`.
pub(crate) async fn run(
    clock: Arc<dyn Clock>,
    pings: Arc<Pings>,
    control: mpsc::Sender<Frame<'static>>,
    close: CloseSlot,
    dead: CancellationToken,
) {
    loop {
        clock.sleep(PING_INTERVAL).await;
        if pings.outstanding() >= MISSED_PONGS {
            tracing::debug!("a WebSocket peer missed its pongs");
            close.set(WebSocketCloseReason::PingTimeout);
            dead.cancel();
            return;
        }
        let number = pings.next();
        let ping =
            Frame::new(true, OpCode::Ping, None, Payload::Owned(number.to_be_bytes().to_vec()));
        match control.try_send(ping) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Closed(_)) => return,
        }
    }
}

#[cfg(test)]
mod tests;
