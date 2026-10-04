//! The single writer: the only read-write connection, owned by one thread.
//!
//! [`StoreWriter::spawn`] moves the connection onto a named thread that runs jobs one
//! at a time from a bounded queue. [`WriterHandle`] is the only way in: it sends a job
//! and awaits the reply. Because jobs run one at a time, commits are serial, and the
//! broadcast that follows each commit is in commit order without a further lock.
//!
//! The writer is a thread rather than a tokio task because every SQLite call blocks.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use efr_protocol::{EventEnvelope, Seq};
use efr_stdx::time::Clock;
use jiff::Timestamp;
use rusqlite::{Connection, TransactionBehavior};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::outbox::{self, OutboxId, OutboxItem, OutboxReconciled};
use crate::{StoreError, events, projection, reader, receipts, sql};

mod batch;

pub use batch::{Batch, Committed};

/// Jobs waiting for the writer; senders wait when it is full, which is the
/// backpressure on writes.
const JOB_QUEUE: usize = 64;

/// SQLite recurses while it plans statements; the default stack of a Rust thread is
/// enough and this keeps it explicit.
const STACK_SIZE: usize = 2 * 1024 * 1024;

const THREAD_NAME: &str = "efr-store-writer";

/// The default number of committed batches that the broadcast keeps for a slow
/// subscriber before it reports the subscriber as lagging.
pub const DEFAULT_BROADCAST_CAPACITY: usize = 1024;

type Job = Box<dyn FnOnce(&mut WriterState) + Send + 'static>;

/// What the writer thread owns.
pub(crate) struct WriterState {
    pub(crate) conn: Connection,
    clock: Arc<dyn Clock>,
    /// The sequence number of the newest committed event.
    last_seq: Seq,
    events: broadcast::Sender<Committed>,
}

/// The running writer thread.
///
/// The thread stops when every [`WriterHandle`] (and every reader built on one) is
/// gone; [`StoreWriter::join`] waits for that, after which the connection is closed
/// and SQLite has checkpointed the write-ahead log.
#[derive(Debug)]
pub struct StoreWriter {
    stopped: oneshot::Receiver<()>,
}

/// A cheap, cloneable handle to the writer: the only way to write to the store.
#[derive(Debug, Clone)]
pub struct WriterHandle {
    jobs: mpsc::Sender<Job>,
    events: broadcast::Sender<Committed>,
}

impl StoreWriter {
    /// Starts the writer thread on `conn`, which must be migrated and must be the only
    /// read-write connection to its database. `broadcast_capacity` bounds how many
    /// committed batches a subscriber may fall behind by.
    pub fn spawn(
        conn: Connection,
        clock: Arc<dyn Clock>,
        broadcast_capacity: usize,
    ) -> Result<(WriterHandle, StoreWriter), StoreError> {
        let last_seq = events::last_seq(&conn)?;
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>(JOB_QUEUE);
        // A broadcast channel of capacity 0 panics.
        let (events_tx, _) = broadcast::channel(broadcast_capacity.max(1));
        let (stopped_tx, stopped_rx) = oneshot::channel();
        let state = WriterState { conn, clock, last_seq, events: events_tx.clone() };
        efr_stdx::thread::spawn_named(THREAD_NAME, STACK_SIZE, move || {
            run(state, jobs_rx);
            // `run` consumed the state, so the connection is closed by now.
            let _ = stopped_tx.send(());
        })
        .map_err(|source| StoreError::SpawnWriter { source })?;
        Ok((WriterHandle { jobs: jobs_tx, events: events_tx }, StoreWriter { stopped: stopped_rx }))
    }

    /// Waits until the writer thread has stopped and closed its connection. It stops
    /// once every handle is dropped, so a caller that still holds one waits forever.
    pub async fn join(self) {
        // An error means the thread ended without sending, which is a stop as well.
        let _ = self.stopped.await;
    }
}

fn run(mut state: WriterState, mut jobs: mpsc::Receiver<Job>) {
    while let Some(job) = jobs.blocking_recv() {
        // A job that panics drops its reply sender, and its caller sees
        // `TaskPanicked`. An open transaction rolls back as the panic unwinds, and the
        // sequence counter only moves after a commit, so the state stays good for the
        // next job.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| job(&mut state)));
    }
}

impl WriterHandle {
    /// Commits `batch` in one transaction and then broadcasts its events.
    ///
    /// Events get consecutive sequence numbers after the newest committed one and all
    /// get the same time from the clock, to the microsecond. A batch that fails writes
    /// nothing and uses up no sequence numbers; one whose command id already has a
    /// receipt fails with [`StoreError::DuplicateCommand`]. A batch without events
    /// broadcasts nothing.
    pub async fn append(&self, batch: Batch) -> Result<Committed, StoreError> {
        self.run(move |state| state.append(batch)).await
    }

    /// A receiver of every batch committed from now on, in commit order.
    ///
    /// To follow the log without gaps or repeats: subscribe, then read the high-water
    /// mark ([`events::last_seq`]) and replay up to it from the log, then forward the
    /// received events above it. A receiver that falls more than the broadcast
    /// capacity behind gets `RecvError::Lagged` and must resubscribe.
    pub fn subscribe(&self) -> broadcast::Receiver<Committed> {
        self.events.subscribe()
    }

    /// Claims up to `limit` waiting outbox items, oldest first. A claimed item is not
    /// handed out again until a restart requeues it.
    pub async fn outbox_claim(&self, limit: u32) -> Result<Vec<OutboxItem>, StoreError> {
        self.run(move |state| state.write(|tx, at| outbox::claim(tx, at, limit))).await
    }

    /// Marks a claimed outbox item done. Fails with
    /// [`StoreError::OutboxItemNotClaimed`] for an item that is not claimed, is
    /// already done or was cancelled.
    pub async fn outbox_done(&self, id: OutboxId) -> Result<(), StoreError> {
        self.run(move |state| state.write(|tx, at| outbox::done(tx, id, at))).await
    }

    /// The outbox step of the startup reconciliation: cancels every unfinished
    /// process-bound item and returns every claimed, unfinished replay-safe item to
    /// the queue. Runs before any worker claims.
    pub async fn outbox_cancel_process_bound(&self) -> Result<OutboxReconciled, StoreError> {
        self.run(|state| state.write(outbox::cancel_process_bound)).await
    }

    /// Throws the projections away and rebuilds them from the event log in one
    /// transaction. They are a function of the log, so the result equals what the
    /// writer built event by event; this is how a projection bug is repaired.
    pub async fn rebuild_projections(&self) -> Result<(), StoreError> {
        self.run(|state| state.write(|tx, _at| projection::rebuild(tx))).await
    }

    /// Runs a read on the writer's connection with `query_only` set, for an in-memory
    /// store whose only connection is the writer's.
    pub(crate) async fn read<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        self.run(move |state| {
            let _query_only = QueryOnly::set(&state.conn)?;
            reader::read_in_transaction(&state.conn, f)
        })
        .await
    }

    /// Runs `job` on the writer thread with the connection and returns its result.
    pub(crate) async fn run<T, F>(&self, job: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut WriterState) -> Result<T, StoreError> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let job: Job = Box::new(move |state| {
            // The caller may have stopped waiting; the work is done either way.
            let _ = reply_tx.send(job(state));
        });
        self.jobs.send(job).await.map_err(|_| StoreError::WriterStopped)?;
        reply_rx.await.map_err(|_| StoreError::TaskPanicked { task: "writer" })?
    }
}

impl WriterState {
    /// Runs `f` in an immediate transaction with the current time, and commits when it
    /// succeeds.
    pub(crate) fn write<T>(
        &mut self,
        f: impl FnOnce(&Connection, Timestamp) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let at = sql::truncate_to_micros(self.clock.now());
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx, at)?;
        tx.commit()?;
        Ok(value)
    }

    fn append(&mut self, batch: Batch) -> Result<Committed, StoreError> {
        if batch.is_empty() {
            return Ok(Committed::new(Vec::new(), self.last_seq));
        }
        let at = sql::truncate_to_micros(self.clock.now());
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut next = self.last_seq.get();
        let mut envelopes = Vec::with_capacity(batch.events.len());
        for (conversation_id, event) in batch.events {
            next += 1;
            let envelope = EventEnvelope { seq: Seq::new(next), conversation_id, at, event };
            events::insert(&tx, &envelope)?;
            projection::apply(&tx, &envelope)?;
            envelopes.push(envelope);
        }
        for receipt in &batch.receipts {
            receipts::record(&tx, receipt, receipt.seq_in(&envelopes)?, at)?;
        }
        for item in &batch.outbox {
            outbox::enqueue(&tx, item, at)?;
        }
        tx.commit()?;
        self.last_seq = Seq::new(next);
        let committed = Committed::new(envelopes, self.last_seq);
        if !committed.events().is_empty() {
            // No receiver is not an error: nobody is subscribed yet.
            let _ = self.events.send(committed.clone());
        }
        Ok(committed)
    }
}

/// Keeps the writer's connection from writing while a read runs on it. Dropping the
/// guard, also while a panic unwinds, makes the connection writable again.
struct QueryOnly<'c>(&'c Connection);

impl<'c> QueryOnly<'c> {
    fn set(conn: &'c Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "query_only", true)?;
        Ok(QueryOnly(conn))
    }
}

impl Drop for QueryOnly<'_> {
    fn drop(&mut self) {
        // Clearing a pragma on an open connection does not fail in practice; if it did,
        // the next write would fail loudly rather than write anything wrong.
        let _ = self.0.pragma_update(None, "query_only", false);
    }
}

#[cfg(test)]
mod tests;
