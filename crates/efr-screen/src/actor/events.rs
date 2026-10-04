//! What a screen thread produces.

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::ShellMark;
use crate::snapshot::ScreenCapture;

/// One thing a screen produced.
///
/// Per feed the actor sends the marks first (in stream order), then bells and title
/// changes in the order they happened, then one [`PtyReply`](ScreenEvent::PtyReply)
/// with every reply byte of that feed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScreenEvent {
    /// Bytes to write to the PTY: the screen's answers to DA, DSR, DECRQM and OSC 10
    /// and 11 queries.
    PtyReply(Bytes),
    /// An OSC 133 or OSC 7 mark, with its recording offsets.
    ShellMark(ShellMark),
    /// The program set the window title.
    TitleChanged(String),
    /// The program rang the bell.
    Bell,
    /// The answer to [`ScreenHandle::request_snapshot`](crate::ScreenHandle::request_snapshot),
    /// in order with the events of the feeds before it.
    Snapshot {
        /// The id the request carried.
        id: u64,
        /// The snapshot.
        capture: ScreenCapture,
    },
}

/// The stream of [`ScreenEvent`]s from one screen.
///
/// It ends (`recv` returns `None`) once the actor has stopped and every event before
/// that was read. The reader must keep reading: while 64 events wait unread, the
/// screen thread waits too, and so do the feeds behind it. Dropping the stream is fine;
/// the actor then discards events.
#[derive(Debug)]
pub struct ScreenEvents {
    events: mpsc::Receiver<ScreenEvent>,
}

impl ScreenEvents {
    pub(crate) fn new(events: mpsc::Receiver<ScreenEvent>) -> Self {
        ScreenEvents { events }
    }

    /// The next event, or `None` when the actor has stopped and the stream is drained.
    /// Cancel-safe, so it may sit in a `tokio::select!`.
    pub async fn recv(&mut self) -> Option<ScreenEvent> {
        self.events.recv().await
    }

    /// The next event, waiting on the current thread, for readers on a plain thread.
    ///
    /// # Panics
    ///
    /// Panics when called inside an async context, as tokio's blocking receive does.
    pub fn blocking_recv(&mut self) -> Option<ScreenEvent> {
        self.events.blocking_recv()
    }
}
