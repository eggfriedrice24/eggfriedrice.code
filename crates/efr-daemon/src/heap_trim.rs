//! The heap trim: one task that gives the freed heap back to the system after the
//! conversation actors dropped their turns (`docs/adr/0020-malloc-trim.md`).
//!
//! An actor that is idle for 10 minutes drops the turns that it keeps in memory and
//! adds one to the count of `ConversationDeps::idle_trims`. glibc keeps most of that
//! freed heap in its arenas, so this task then calls the [`Trimmer`] (efrd's is
//! [`GlibcTrimmer`], `malloc_trim(0)` of `malloc.rs`) once, on a blocking thread:
//!
//! - after an actor trim, when [`QUIET`] passed on the clock with no turn running, no
//!   turn end and no other actor trim, so many actors that trim close together give
//!   one heap trim;
//! - never while a turn runs: the task follows the turns from the committed batches
//!   (`turn_started` and the four turn ends), and a turn that starts in the quiet
//!   period stops the wait;
//! - at most once in [`MIN_GAP`].
//!
//! Without an actor trim the task holds no timer. When the broadcast of the committed
//! batches lags, the task reads the running turns from the store again, and the quiet
//! period starts again. After each step it publishes a [`HeapTrimStatus`] on a `watch`,
//! which the tests wait on.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{Event, TurnId};
use efr_stdx::time::{Clock, Sleep};
use efr_store::conversations::TurnStatus;
use efr_store::{Committed, Readers};
use jiff::Timestamp;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// How long no turn runs or ends and no actor trims before the heap trim.
pub(crate) const QUIET: Duration = Duration::from_secs(30);

/// The shortest time between two heap trims.
pub(crate) const MIN_GAP: Duration = Duration::from_secs(5 * 60);

/// Gives the freed heap back to the system. efrd's is [`GlibcTrimmer`]; a test's
/// counts the calls.
pub(crate) trait Trimmer: Send + Sync + 'static {
    /// Gives the free heap back; true when the allocator gave memory back. It may
    /// block for some milliseconds.
    fn trim(&self) -> bool;
}

/// `malloc_trim(0)` of glibc (`malloc.rs`); nothing on a target without glibc.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GlibcTrimmer;

impl Trimmer for GlibcTrimmer {
    fn trim(&self) -> bool {
        crate::malloc::trim()
    }
}

/// What the task has seen and done, published after each step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HeapTrimStatus {
    /// The count of the actors' idle trims when the task last saw it change.
    pub(crate) actor_trims: u64,
    /// The turns that run now, as the committed batches tell.
    pub(crate) running: usize,
    /// When the next heap trim comes; `None` when none is due (no actor trim since the
    /// last one, or a turn runs).
    pub(crate) due: Option<Timestamp>,
    /// The heap trims since the start.
    pub(crate) trims: u64,
}

/// A new status channel, with nothing seen yet.
pub(crate) fn status_channel() -> (watch::Sender<HeapTrimStatus>, watch::Receiver<HeapTrimStatus>) {
    watch::channel(HeapTrimStatus::default())
}

/// The inputs of the task.
pub(crate) struct HeapTrimParts {
    /// The count of the actors' idle trims (`ConversationDeps::idle_trims`).
    pub(crate) actor_trims: watch::Receiver<u64>,
    /// The committed batches of the store's writer.
    pub(crate) committed: broadcast::Receiver<Committed>,
    /// The store's readers, for the running turns after a lag.
    pub(crate) readers: Readers,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) status: watch::Sender<HeapTrimStatus>,
    pub(crate) stop: CancellationToken,
}

/// Gives the freed heap back with `trimmer` after the actors trim, until `stop`.
pub(crate) async fn follow<T: Trimmer>(parts: HeapTrimParts, trimmer: Arc<T>) {
    HeapTrim::new(parts, trimmer).run().await;
}

/// The running task.
struct HeapTrim<T> {
    parts: HeapTrimParts,
    trimmer: Arc<T>,
    running: HashSet<TurnId>,
    /// True when an actor trimmed since the last heap trim.
    pending: bool,
    /// The last actor trim, turn end or lag: the quiet period counts from it.
    quiet_since: Timestamp,
    /// The last heap trim.
    last_trim: Option<Timestamp>,
    status: HeapTrimStatus,
}

impl<T: Trimmer> HeapTrim<T> {
    fn new(parts: HeapTrimParts, trimmer: Arc<T>) -> Self {
        // NOTE: the receiver keeps the version that it saw last, from the start of the
        // channel on, so an actor trim before the first poll of this task still wakes
        // it. Marking the count as seen here lost such a trim, and the actors' memory
        // then waited for the next one.
        let quiet_since = parts.clock.now();
        HeapTrim {
            parts,
            trimmer,
            running: HashSet::new(),
            pending: false,
            quiet_since,
            last_trim: None,
            status: HeapTrimStatus::default(),
        }
    }

    async fn run(mut self) {
        loop {
            let due = self.due();
            self.publish(due);
            let sleep = due.map(|at| self.parts.clock.sleep(self.until(at)));
            tokio::select! {
                biased;
                () = self.parts.stop.cancelled() => return,
                changed = self.parts.actor_trims.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    self.status.actor_trims = *self.parts.actor_trims.borrow_and_update();
                    self.pending = true;
                    self.quiet_since = self.parts.clock.now();
                }
                batch = self.parts.committed.recv() => match batch {
                    Ok(batch) => self.see(&batch),
                    Err(RecvError::Lagged(_)) => self.lagged().await,
                    Err(RecvError::Closed) => return,
                },
                () = wait(sleep) => self.trim().await,
            }
        }
    }

    /// When the next heap trim comes: [`QUIET`] after the last actor trim or turn end,
    /// and [`MIN_GAP`] after the last heap trim; `None` when no actor trimmed since
    /// the last one or a turn runs.
    fn due(&self) -> Option<Timestamp> {
        if !self.pending || !self.running.is_empty() {
            return None;
        }
        let quiet = self.quiet_since.checked_add(QUIET).ok()?;
        match self.last_trim {
            Some(last) => Some(quiet.max(last.checked_add(MIN_GAP).ok()?)),
            None => Some(quiet),
        }
    }

    fn see(&mut self, batch: &Committed) {
        for envelope in batch.events() {
            match &envelope.event {
                Event::TurnStarted { turn_id, .. } => {
                    self.running.insert(*turn_id);
                }
                Event::TurnCompleted { turn_id, .. }
                | Event::TurnFailed { turn_id, .. }
                | Event::TurnInterrupted { turn_id, .. }
                | Event::TurnCancelled { turn_id } => {
                    self.running.remove(turn_id);
                    self.quiet_since = self.parts.clock.now();
                }
                _ => {}
            }
        }
    }

    /// The broadcast lost batches: the running turns are read from the store again, and
    /// the quiet period starts again. A failed read keeps the turns that the task knew.
    async fn lagged(&mut self) {
        self.quiet_since = self.parts.clock.now();
        let read = self.parts.readers.with(efr_store::conversations::unfinished_turns).await;
        match read {
            Ok(turns) => {
                self.running = turns
                    .into_iter()
                    .filter(|turn| turn.status == TurnStatus::Running)
                    .map(|turn| turn.id)
                    .collect();
            }
            Err(error) => {
                tracing::warn!(error = %error, "the heap trim could not read the running turns");
            }
        }
    }

    async fn trim(&mut self) {
        let trimmer = Arc::clone(&self.trimmer);
        let released = tokio::task::spawn_blocking(move || trimmer.trim()).await;
        self.pending = false;
        self.last_trim = Some(self.parts.clock.now());
        self.status.trims += 1;
        match released {
            // NOTE: `bench_perf_daemon_memory` counts this event by its target and its
            // `released` field.
            Ok(released) => tracing::debug!(released, "efrd gave the freed heap back"),
            Err(_) => tracing::warn!("the heap trim panicked"),
        }
    }

    fn until(&self, at: Timestamp) -> Duration {
        Duration::try_from(at.duration_since(self.parts.clock.now())).unwrap_or_default()
    }

    fn publish(&mut self, due: Option<Timestamp>) {
        self.status.running = self.running.len();
        self.status.due = due;
        let status = self.status;
        self.parts.status.send_if_modified(|published| {
            let changed = *published != status;
            *published = status;
            changed
        });
    }
}

/// The end of `sleep`; never, when there is none.
async fn wait(sleep: Option<Sleep>) {
    match sleep {
        Some(sleep) => sleep.await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
