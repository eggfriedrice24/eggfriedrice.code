//! The only way other crates talk to a screen.

use std::fmt;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, TrySendError};

use bytes::Bytes;
use efr_protocol::{Seq, Size};
use tokio::sync::{Notify, oneshot};

use super::{ScreenCommand, SnapshotReply};
use crate::ScreenError;
use crate::snapshot::ScreenCapture;

/// Sends commands to one screen thread. Cheap to clone; every clone talks to the same
/// screen, and the screen stops when the last clone is dropped.
///
/// The async methods park when the command queue is full and resume when the screen
/// takes a command, without blocking a tokio worker. The `_blocking` methods wait on
/// the current thread and are for plain threads only (the PTY proxy, the conformance
/// suite); called from an async task they would stall a worker.
///
/// Every method but [`name`](Self::name) fails with [`ScreenError::Closed`] once the
/// actor has stopped.
#[derive(Clone)]
pub struct ScreenHandle {
    name: Arc<str>,
    commands: SyncSender<ScreenCommand>,
    /// Signalled by the actor whenever it takes a command or stops.
    space: Arc<Notify>,
}

impl ScreenHandle {
    pub(crate) fn new(
        name: String,
        commands: SyncSender<ScreenCommand>,
        space: Arc<Notify>,
    ) -> Self {
        ScreenHandle { name: Arc::from(name), commands, space }
    }

    /// The name of the screen thread.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Queues PTY output whose first byte is at recording offset `base_seq`. Chunks
    /// of one stream must be fed in order; a chunk whose `base_seq` does not continue
    /// the previous one tells the mark scanner that bytes are missing.
    pub async fn feed(&self, bytes: Bytes, base_seq: Seq) -> Result<(), ScreenError> {
        self.send(ScreenCommand::Feed { bytes, base_seq }).await
    }

    /// [`feed`](Self::feed) for plain threads.
    pub fn feed_blocking(&self, bytes: Bytes, base_seq: Seq) -> Result<(), ScreenError> {
        self.send_blocking(ScreenCommand::Feed { bytes, base_seq })
    }

    /// Queues a change of the grid size.
    pub async fn resize(&self, size: Size) -> Result<(), ScreenError> {
        self.send(ScreenCommand::Resize { size }).await
    }

    /// [`resize`](Self::resize) for plain threads.
    pub fn resize_blocking(&self, size: Size) -> Result<(), ScreenError> {
        self.send_blocking(ScreenCommand::Resize { size })
    }

    /// A snapshot with up to `scrollback_rows` rows of scrollback, taken after every
    /// command queued before it.
    ///
    /// NOTE: the task that reads this screen's [`ScreenEvents`](crate::ScreenEvents)
    /// must not await this. While that task waits here it reads no events, so once the
    /// event queue is full the screen waits for it and never reaches the snapshot. Use
    /// [`request_snapshot`](Self::request_snapshot) there instead.
    pub async fn snapshot(&self, scrollback_rows: usize) -> Result<ScreenCapture, ScreenError> {
        let (reply, answer) = oneshot::channel();
        self.send(ScreenCommand::Snapshot { scrollback_rows, reply: SnapshotReply::Caller(reply) })
            .await?;
        answer.await.map_err(|_| self.closed())
    }

    /// [`snapshot`](Self::snapshot) for plain threads.
    ///
    /// # Panics
    ///
    /// Panics when called inside an async context, as tokio's blocking receive does.
    pub fn snapshot_blocking(&self, scrollback_rows: usize) -> Result<ScreenCapture, ScreenError> {
        let (reply, answer) = oneshot::channel();
        self.send_blocking(ScreenCommand::Snapshot {
            scrollback_rows,
            reply: SnapshotReply::Caller(reply),
        })?;
        answer.blocking_recv().map_err(|_| self.closed())
    }

    /// Asks for a snapshot that arrives on the event stream as
    /// [`ScreenEvent::Snapshot`](crate::ScreenEvent::Snapshot) with this `id`, after
    /// the events of every feed queued before it. This is the way for the event reader
    /// itself to get a snapshot, and a barrier: once it arrives, every earlier event
    /// has been read.
    pub async fn request_snapshot(
        &self,
        id: u64,
        scrollback_rows: usize,
    ) -> Result<(), ScreenError> {
        self.send(ScreenCommand::Snapshot { scrollback_rows, reply: SnapshotReply::Event(id) })
            .await
    }

    /// [`request_snapshot`](Self::request_snapshot) for plain threads.
    pub fn request_snapshot_blocking(
        &self,
        id: u64,
        scrollback_rows: usize,
    ) -> Result<(), ScreenError> {
        self.send_blocking(ScreenCommand::Snapshot {
            scrollback_rows,
            reply: SnapshotReply::Event(id),
        })
    }

    /// Stops the actor after the commands queued before this one. The screen is
    /// dropped on its thread and the event stream ends.
    pub async fn shutdown(&self) -> Result<(), ScreenError> {
        self.send(ScreenCommand::Shutdown).await
    }

    /// [`shutdown`](Self::shutdown) for plain threads.
    pub fn shutdown_blocking(&self) -> Result<(), ScreenError> {
        self.send_blocking(ScreenCommand::Shutdown)
    }

    async fn send(&self, mut command: ScreenCommand) -> Result<(), ScreenError> {
        loop {
            // Registered before the attempt, so a slot freed between a failed attempt
            // and the wait still wakes this sender.
            let mut space = pin!(self.space.notified());
            space.as_mut().enable();
            match self.commands.try_send(command) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(back)) => {
                    command = back;
                    space.await;
                }
                Err(TrySendError::Disconnected(_)) => return Err(self.closed()),
            }
        }
    }

    fn send_blocking(&self, command: ScreenCommand) -> Result<(), ScreenError> {
        self.commands.send(command).map_err(|_| self.closed())
    }

    fn closed(&self) -> ScreenError {
        ScreenError::Closed { name: self.name.to_string() }
    }
}

impl fmt::Debug for ScreenHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenHandle").field("name", &self.name).finish_non_exhaustive()
    }
}
