//! The `RecordingSink` over the store's recordings.
//!
//! Each PTY gets a writer task that owns its `RecordingWriter`, so appends to one
//! recording are serial and never block a tokio worker. `record` waits until its chunk
//! is stored, which is the backpressure from a slow disk back to the shell's reader.
//! After each chunk the task hands it to the live `pty.attach` streams.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use bytes::Bytes;
use efr_protocol::{PtyId, Seq};
use efr_shell::RecordingSink;
use efr_store::recording::{RecordingWriter, Recordings};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::ptys::Ptys;

/// Chunks waiting for one PTY's writer.
const CHUNK_QUEUE: usize = 16;

#[derive(Debug)]
struct Chunk {
    start: Seq,
    bytes: Bytes,
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
        let (stored, done) = oneshot::channel();
        if self.sender(pty_id).send(Chunk { start, bytes, stored }).await.is_err() {
            tracing::warn!(%pty_id, "the recording writer stopped; a chunk was not stored");
            return;
        }
        // The writer answers after it stored the chunk, or drops the sender if it died.
        let _ = done.await;
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
    while let Some(Chunk { start, bytes, stored }) = chunks.recv().await {
        let at = match writer.as_mut() {
            Some(writer) => append(writer, start, &bytes).await,
            None => start,
        };
        ptys.recorded(pty_id, at.get(), bytes);
        let _ = stored.send(());
    }
    if let Some(writer) = writer
        && let Err(error) = writer.close().await
    {
        tracing::warn!(error = %error, %pty_id, "the recording could not be closed");
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
