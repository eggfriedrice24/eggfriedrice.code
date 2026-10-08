//! The `RecordingSink` over the store's recordings.
//!
//! Each PTY gets a writer task that owns its `RecordingWriter`, so appends to one
//! recording are serial and never block a tokio worker. `record` waits until its chunk
//! is stored, which is the backpressure from a slow disk back to the shell's reader.
//! A new size of the PTY goes through the same task, so the recording holds it between
//! the chunks before and after it. After each chunk or size the task hands it to the
//! live `pty.attach` streams.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use bytes::Bytes;
use efr_protocol::{PtyId, Seq, Size};
use efr_shell::RecordingSink;
use efr_store::recording::{RecordingWriter, Recordings};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::ptys::Ptys;

/// Steps waiting for one PTY's writer.
const CHUNK_QUEUE: usize = 16;

/// One step of a PTY for its writer.
#[derive(Debug)]
enum Step {
    /// Output that the shell's reader read at its offset `start`.
    Output { start: Seq, bytes: Bytes },
    /// A new size.
    Resize { size: Size },
}

#[derive(Debug)]
struct Chunk {
    step: Step,
    stored: oneshot::Sender<()>,
}

#[derive(Debug)]
struct Writer {
    chunks: mpsc::Sender<Chunk>,
    task: JoinHandle<()>,
}

#[derive(Debug, Default)]
struct Writers {
    open: HashMap<PtyId, Writer>,
    /// PTYs whose shell exited. Output the reader still drains after the exit gets a
    /// writer of its own that closes after the chunk, so nothing is left open.
    closed: HashSet<PtyId>,
}

/// Stores every PTY's bytes and fans them out to attached clients.
#[derive(Debug)]
pub(crate) struct StoreRecording {
    recordings: Recordings,
    ptys: Arc<Ptys>,
    writers: Mutex<Writers>,
}

impl StoreRecording {
    pub(crate) fn new(recordings: Recordings, ptys: Arc<Ptys>) -> Self {
        StoreRecording { recordings, ptys, writers: Mutex::default() }
    }

    /// Closes the recording of `pty_id` once the chunks already sent are stored.
    pub(crate) fn close(&self, pty_id: PtyId) {
        // Dropping the sender ends the task after its queue, and the task closes the
        // segment. Nothing waits for it here: the observer must not block.
        let mut writers = self.lock();
        writers.closed.insert(pty_id);
        drop(writers.open.remove(&pty_id));
    }

    /// Stores that `pty_id` took `size` and then tells the attached clients, after the
    /// output stored before it. Returns once it is stored, or when the writer stopped.
    pub(crate) async fn resized(&self, pty_id: PtyId, size: Size) {
        if self.lock().closed.contains(&pty_id) {
            return;
        }
        self.send(pty_id, Step::Resize { size }).await;
    }

    /// Hands `step` to the writer of `pty_id` and waits until it is stored.
    async fn send(&self, pty_id: PtyId, step: Step) {
        let (stored, done) = oneshot::channel();
        if self.sender(pty_id).send(Chunk { step, stored }).await.is_err() {
            tracing::warn!(%pty_id, "the recording writer stopped; a chunk was not stored");
            return;
        }
        // The writer answers after it stored the step, or drops the sender if it died.
        let _ = done.await;
    }

    /// Closes every recording and waits until each segment is flushed and closed.
    pub(crate) async fn close_all(&self) {
        let writers: Vec<Writer> = self.lock().open.drain().map(|(_, writer)| writer).collect();
        for Writer { chunks, task } in writers {
            drop(chunks);
            if task.await.is_err() {
                tracing::warn!("a recording writer panicked");
            }
        }
    }

    fn sender(&self, pty_id: PtyId) -> mpsc::Sender<Chunk> {
        let mut writers = self.lock();
        if let Some(writer) = writers.open.get(&pty_id).filter(|writer| !writer.chunks.is_closed())
        {
            return writer.chunks.clone();
        }
        let (chunks, queue) = mpsc::channel(CHUNK_QUEUE);
        let task = tokio::spawn(write_loop(
            self.recordings.clone(),
            Arc::clone(&self.ptys),
            pty_id,
            queue,
        ));
        if !writers.closed.contains(&pty_id) {
            writers.open.insert(pty_id, Writer { chunks: chunks.clone(), task });
        }
        chunks
    }

    fn lock(&self) -> MutexGuard<'_, Writers> {
        // Every critical section leaves the map whole, so a poisoned lock is still good.
        self.writers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl RecordingSink for StoreRecording {
    async fn record(&self, pty_id: PtyId, start: Seq, bytes: Bytes) {
        self.send(pty_id, Step::Output { start, bytes }).await;
    }
}

/// Owns one PTY's recording writer until its sender is dropped.
async fn write_loop(
    recordings: Recordings,
    ptys: Arc<Ptys>,
    pty_id: PtyId,
    mut chunks: mpsc::Receiver<Chunk>,
) {
    let mut writer = match recordings.start(pty_id).await {
        Ok(writer) => Some(writer),
        Err(error) => {
            tracing::error!(error = %error, %pty_id, "the recording could not start; output is shown but not kept");
            None
        }
    };
    // The offset after the last output, for a size when the recording failed.
    let mut end = 0;
    while let Some(Chunk { step, stored }) = chunks.recv().await {
        match step {
            Step::Output { start, bytes } => {
                let at = match writer.as_mut() {
                    Some(writer) => append(writer, start, &bytes).await,
                    None => start,
                };
                let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                end = at.get().saturating_add(len);
                ptys.recorded(pty_id, at.get(), bytes);
            }
            Step::Resize { size } => {
                let at = match writer.as_mut() {
                    Some(writer) => resize(writer, size, end).await,
                    None => end,
                };
                ptys.resized(pty_id, at, size);
            }
        }
        let _ = stored.send(());
    }
    if let Some(writer) = writer
        && let Err(error) = writer.close().await
    {
        tracing::warn!(error = %error, %pty_id, "the recording could not be closed");
    }
}

/// Stores `size` and returns its recording offset, or `end` when that failed.
async fn resize(writer: &mut RecordingWriter, size: Size, end: u64) -> u64 {
    match writer.resize(size).await {
        Ok(at) => at.get(),
        Err(error) => {
            tracing::warn!(error = %error, pty_id = %writer.pty_id(), "a size could not be stored");
            end
        }
    }
}

/// Appends `bytes` and returns their recording offset, or the shell's offset when the
/// append failed.
async fn append(writer: &mut RecordingWriter, start: Seq, bytes: &[u8]) -> Seq {
    match writer.append(bytes).await {
        Ok(first) => {
            if first != start {
                tracing::debug!(%start, recorded = %first, "the recording offset differs from the shell's");
            }
            first
        }
        Err(error) => {
            tracing::warn!(error = %error, pty_id = %writer.pty_id(), "a chunk could not be stored");
            start
        }
    }
}
