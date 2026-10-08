//! The hidden shells' PTYs as the protocol sees them: which conversation each belongs
//! to, how far its recording reaches, how much input it took, and the live fan-out of
//! its output to `pty.attach` streams.
//!
//! The recording sink reports every chunk and every new size here after it stored it, so
//! a client that registers before it reads the recording misses nothing: a step is
//! either in what it read or offered to it live, or both. The registration returns the
//! [`AttachMark`] of that moment, and the attach handler replays the recording only up
//! to it: what comes after, the client gets live, so nothing comes twice. Each attached
//! client has its own bounded queue; a full queue closes that client's stream with
//! `overflow` and never slows the shell.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use efr_protocol::{ConversationId, PtyId, Size};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// How many live items an attached client may have queued before its stream is closed.
pub(crate) const ATTACH_QUEUE: usize = 64;

/// One live step of a PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PtyLive {
    /// Output whose first byte is at recording offset `start`.
    Output { start: u64, data: Bytes },
    /// The PTY took a new size at recording offset `at`.
    Resized { at: u64, size: Size },
}

/// What an attached client receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PtyDelivery {
    /// The next live step.
    Live(PtyLive),
    /// The client fell behind and its stream is closed.
    Overflowed,
}

/// How far the steps offered before a client registered reach: every output byte
/// before `end`, and the first `sizes_at_end` sizes at `end`. Everything else the client
/// gets live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AttachMark {
    pub(crate) end: u64,
    pub(crate) sizes_at_end: u32,
}

/// The live side of one attached client.
#[derive(Debug)]
pub(crate) struct AttachReceiver {
    rx: mpsc::Receiver<PtyLive>,
    overflowed: Arc<AtomicBool>,
    mark: AttachMark,
}

impl AttachReceiver {
    /// What was offered before this client registered.
    pub(crate) fn mark(&self) -> AttachMark {
        self.mark
    }

    /// The next step, or `None` once the shell exited and everything queued was read.
    pub(crate) async fn recv(&mut self) -> Option<PtyDelivery> {
        match self.rx.recv().await {
            Some(live) => Some(PtyDelivery::Live(live)),
            None if self.overflowed.load(Ordering::Acquire) => Some(PtyDelivery::Overflowed),
            None => None,
        }
    }
}

#[derive(Debug)]
struct AttachSender {
    tx: mpsc::Sender<PtyLive>,
    overflowed: Arc<AtomicBool>,
}

impl AttachSender {
    /// Queues `live` without waiting; false when this client is gone or fell behind.
    fn offer(&self, live: PtyLive) -> bool {
        match self.tx.try_send(live) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                // NOTE: stored before the sender is dropped, so the receiver finds it set
                // when it sees the channel close.
                self.overflowed.store(true, Ordering::Release);
                false
            }
            Err(TrySendError::Closed(_)) => false,
        }
    }
}

#[derive(Debug)]
struct Entry {
    conversation: ConversationId,
    /// The shell's process id, when its start was reported.
    pid: Option<u32>,
    /// The recording offset after the last stored byte.
    end: u64,
    /// The sizes stored at `end`.
    sizes_at_end: u32,
    /// Bytes typed into the PTY through `pty.write`.
    input: u64,
    attached: Vec<AttachSender>,
}

impl Entry {
    /// Moves the end to `end` when that is further.
    fn advance(&mut self, end: u64) {
        if end > self.end {
            self.end = end;
            self.sizes_at_end = 0;
        }
    }
}

/// What the idle collector sees of one PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PtyActivity {
    pub(crate) pty_id: PtyId,
    pub(crate) conversation: ConversationId,
    /// Output and input so far: unchanged means nothing happened.
    pub(crate) mark: u64,
    /// True while a client is attached.
    pub(crate) attached: bool,
}

/// Every running hidden shell's PTY.
#[derive(Debug, Default)]
pub(crate) struct Ptys {
    entries: Mutex<HashMap<PtyId, Entry>>,
}

impl Ptys {
    /// A shell of `conversation` started on `pty_id` as the process `pid`.
    pub(crate) fn started(&self, pty_id: PtyId, conversation: ConversationId, pid: Option<u32>) {
        let mut entries = self.lock();
        let entry = entries.entry(pty_id).or_insert_with(|| Entry {
            conversation,
            pid,
            end: 0,
            sizes_at_end: 0,
            input: 0,
            attached: Vec::new(),
        });
        entry.conversation = conversation;
        entry.pid = pid.or(entry.pid);
    }

    /// The process ids of the running hidden shells, for the check of model-side
    /// peers on the socket.
    pub(crate) fn shell_pids(&self) -> Vec<u32> {
        self.lock().values().filter_map(|entry| entry.pid).collect()
    }

    /// The shell on `pty_id` exited: attached streams end after what they have queued.
    pub(crate) fn exited(&self, pty_id: PtyId) {
        self.lock().remove(&pty_id);
    }

    /// The conversation whose shell runs on `pty_id`.
    pub(crate) fn conversation(&self, pty_id: PtyId) -> Option<ConversationId> {
        self.lock().get(&pty_id).map(|entry| entry.conversation)
    }

    /// The running shells' PTYs.
    pub(crate) fn count(&self) -> usize {
        self.lock().len()
    }

    /// The bytes `data`, stored at offset `start`, go to every attached client.
    pub(crate) fn recorded(&self, pty_id: PtyId, start: u64, data: Bytes) {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(&pty_id) else {
            return;
        };
        let len = u64::try_from(data.len()).unwrap_or(u64::MAX);
        entry.advance(start.saturating_add(len));
        let live = PtyLive::Output { start, data };
        entry.attached.retain(|client| client.offer(live.clone()));
    }

    /// `bytes` bytes were typed into `pty_id`.
    pub(crate) fn written(&self, pty_id: PtyId, bytes: u64) {
        if let Some(entry) = self.lock().get_mut(&pty_id) {
            entry.input = entry.input.saturating_add(bytes);
        }
    }

    /// `pty_id` took `size`, stored at recording offset `at`; attached clients learn it.
    pub(crate) fn resized(&self, pty_id: PtyId, at: u64, size: Size) {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(&pty_id) else {
            return;
        };
        entry.advance(at);
        if at == entry.end {
            entry.sizes_at_end = entry.sizes_at_end.saturating_add(1);
        }
        let live = PtyLive::Resized { at, size };
        entry.attached.retain(|client| client.offer(live.clone()));
    }

    /// Registers a client for the live output of `pty_id`. `None` when no shell runs
    /// on it.
    pub(crate) fn attach(&self, pty_id: PtyId) -> Option<AttachReceiver> {
        let mut entries = self.lock();
        let entry = entries.get_mut(&pty_id)?;
        let (tx, rx) = mpsc::channel(ATTACH_QUEUE);
        let overflowed = Arc::new(AtomicBool::new(false));
        entry.attached.push(AttachSender { tx, overflowed: Arc::clone(&overflowed) });
        let mark = AttachMark { end: entry.end, sizes_at_end: entry.sizes_at_end };
        Some(AttachReceiver { rx, overflowed, mark })
    }

    /// The activity of every running PTY, for the idle collector.
    pub(crate) fn activity(&self) -> Vec<PtyActivity> {
        let mut entries = self.lock();
        entries
            .iter_mut()
            .map(|(pty_id, entry)| {
                entry.attached.retain(|client| !client.tx.is_closed());
                PtyActivity {
                    pty_id: *pty_id,
                    conversation: entry.conversation,
                    mark: entry.end.saturating_add(entry.input),
                    attached: !entry.attached.is_empty(),
                }
            })
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PtyId, Entry>> {
        // Every critical section leaves the map whole, so a poisoned lock is still good.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
