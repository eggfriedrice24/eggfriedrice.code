//! One screen per named std thread.
//!
//! libghostty-vt types are neither `Send` nor `Sync`, and their callbacks run inside
//! `vt_write` on the calling thread, so a screen cannot live in a tokio task.
//! [`ScreenActor::spawn`] starts a thread, builds the screen there from a `Send`
//! factory and serves commands from a bounded `std::sync::mpsc::sync_channel`.
//! Everything the screen produces leaves as [`ScreenEvent`]s on a bounded tokio
//! channel. [`ScreenHandle`] and [`ScreenEvents`] are the only types other crates hold.

mod events;
mod handle;

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, sync_channel};

use bytes::Bytes;
use efr_protocol::{Seq, Size};
use tokio::sync::{Notify, mpsc, oneshot};

pub use self::events::{ScreenEvent, ScreenEvents};
pub use self::handle::ScreenHandle;
use crate::sink::{Collector, Notice};
use crate::snapshot::{ScreenCapture, normalize};
use crate::{Screen, ScreenError, ShellMarkScanner};

/// The stack of a screen thread. The actor loop is shallow, and Linux commits stack
/// pages lazily, so N+1 screens cost little more than their grids.
pub const SCREEN_STACK_SIZE: usize = 512 * 1024;

/// How many commands may wait for a screen before a sender parks.
pub(crate) const COMMAND_CAPACITY: usize = 64;

/// How many events may wait for their reader before the screen thread waits.
pub(crate) const EVENT_CAPACITY: usize = 64;

/// What a [`ScreenHandle`] asks of its screen.
#[derive(Debug)]
pub(crate) enum ScreenCommand {
    /// Process PTY output whose first byte is at recording offset `base_seq`.
    Feed { bytes: Bytes, base_seq: Seq },
    /// Change the size of the grid.
    Resize { size: Size },
    /// Take a snapshot with up to `scrollback_rows` rows of scrollback.
    Snapshot { scrollback_rows: usize, reply: SnapshotReply },
    /// Stop the actor; commands queued behind this one are dropped.
    Shutdown,
}

/// Where a snapshot goes.
#[derive(Debug)]
pub(crate) enum SnapshotReply {
    /// Straight back to the caller that waits for it.
    Caller(oneshot::Sender<ScreenCapture>),
    /// Onto the event stream as [`ScreenEvent::Snapshot`] with this id, in order with
    /// the other events.
    Event(u64),
}

/// The owner of one screen and the loop that serves it.
///
/// It exists only on its own thread; [`ScreenActor::spawn`] is the way in.
pub struct ScreenActor<S> {
    screen: S,
    scanner: ShellMarkScanner,
    sink: Collector,
    events: Outbox,
    /// The recording offset after the last byte fed.
    next: Seq,
}

impl<S: Screen + 'static> ScreenActor<S> {
    /// Starts the screen thread `name` and returns the handle that commands it and the
    /// stream of what it produces.
    ///
    /// `factory` runs on the new thread, so the screen it builds never crosses a
    /// thread boundary and needs no `Send`. The screen is then resized to `size`, so it
    /// starts at the size its owner asked for even when the factory built it at
    /// another one. Name the thread after its owner (`screen-<conversation short id>`)
    /// so `top -H` and panic messages say whose screen it is.
    pub fn spawn<F>(
        name: impl Into<String>,
        factory: F,
        size: Size,
    ) -> Result<(ScreenHandle, ScreenEvents), ScreenError>
    where
        F: FnOnce() -> S + Send + 'static,
    {
        let name = name.into();
        let (commands, inbox) = sync_channel(COMMAND_CAPACITY);
        let (events, event_reader) = mpsc::channel(EVENT_CAPACITY);
        let space = Arc::new(Notify::new());
        let inbox = Inbox { commands: Some(inbox), space: Arc::clone(&space) };
        efr_stdx::thread::spawn_named(name.clone(), SCREEN_STACK_SIZE, move || {
            let actor = ScreenActor {
                screen: factory(),
                scanner: ShellMarkScanner::new(),
                sink: Collector::default(),
                events: Outbox { events, open: true },
                next: Seq::ZERO,
            };
            actor.run(inbox, size);
        })
        .map_err(|source| ScreenError::Spawn { name: name.clone(), source })?;
        Ok((ScreenHandle::new(name, commands, space), ScreenEvents::new(event_reader)))
    }
}

impl<S: Screen> ScreenActor<S> {
    fn run(mut self, inbox: Inbox, size: Size) {
        self.resize(size);
        while let Some(command) = inbox.recv() {
            match command {
                ScreenCommand::Feed { bytes, base_seq } => self.feed(&bytes, base_seq),
                ScreenCommand::Resize { size } => self.resize(size),
                ScreenCommand::Snapshot { scrollback_rows, reply } => {
                    self.snapshot(scrollback_rows, reply);
                }
                ScreenCommand::Shutdown => break,
            }
        }
    }

    /// Scans for marks before the backend sees the bytes, so marks are the same for
    /// every backend, then sends the marks, the notices in the order they happened and
    /// the replies the backend buffered.
    fn feed(&mut self, bytes: &[u8], base_seq: Seq) {
        let marks = self.scanner.scan(bytes, base_seq);
        self.screen.feed(bytes, &mut self.sink);
        self.next = Seq::new(base_seq.get().saturating_add(bytes.len() as u64));
        for mark in marks {
            self.events.send(ScreenEvent::ShellMark(mark));
        }
        self.flush();
    }

    fn resize(&mut self, size: Size) {
        self.screen.resize(size.cols, size.rows, &mut self.sink);
        self.flush();
    }

    fn snapshot(&mut self, scrollback_rows: usize, reply: SnapshotReply) {
        let snapshot = normalize(self.screen.snapshot(scrollback_rows), scrollback_rows);
        let capture = ScreenCapture { at: self.next, snapshot };
        match reply {
            // A caller that stopped waiting needs nothing more.
            SnapshotReply::Caller(caller) => drop(caller.send(capture)),
            SnapshotReply::Event(id) => self.events.send(ScreenEvent::Snapshot { id, capture }),
        }
    }

    /// Turns what the sink buffered during one command into events.
    fn flush(&mut self) {
        for notice in self.sink.take_notices() {
            self.events.send(match notice {
                Notice::Bell => ScreenEvent::Bell,
                Notice::Title(title) => ScreenEvent::TitleChanged(title),
            });
        }
        let replies = self.sink.take_replies();
        if !replies.is_empty() {
            self.events.send(ScreenEvent::PtyReply(Bytes::from(replies)));
        }
    }
}

impl<S> fmt::Debug for ScreenActor<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenActor").field("next", &self.next).finish_non_exhaustive()
    }
}

/// The command side as the screen thread sees it.
struct Inbox {
    /// Always `Some` until the inbox is dropped; see the `Drop` impl.
    commands: Option<Receiver<ScreenCommand>>,
    /// Signalled whenever a slot frees up, so async senders parked on a full queue
    /// retry.
    space: Arc<Notify>,
}

impl Inbox {
    /// The next command, or `None` when every handle is gone.
    fn recv(&self) -> Option<ScreenCommand> {
        let command = self.commands.as_ref()?.recv().ok()?;
        self.space.notify_waiters();
        Some(command)
    }
}

impl Drop for Inbox {
    // Also runs while the thread unwinds from a backend panic. The receiver goes first,
    // so a woken sender's retry sees the queue closed instead of parking again.
    fn drop(&mut self) {
        drop(self.commands.take());
        self.space.notify_waiters();
    }
}

/// The event side as the screen thread sees it.
struct Outbox {
    events: mpsc::Sender<ScreenEvent>,
    /// False once the reader is gone; events are then dropped.
    open: bool,
}

impl Outbox {
    /// Waits while the reader is behind: that is the backpressure path back to the
    /// PTY reader. The screen thread is a plain thread, never inside a runtime, so a
    /// blocking send is allowed here.
    fn send(&mut self, event: ScreenEvent) {
        if self.open && self.events.blocking_send(event).is_err() {
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests;
