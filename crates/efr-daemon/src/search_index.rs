//! The search indexer: one background task that keeps the search index of the store
//! current with the event log, off the path of every turn.
//!
//! The index has its cursor in the database (`search_state.indexed_seq`), so a crash
//! or a restart loses nothing: the next start goes on from the cursor. One round reads
//! the next page of source texts after the cursor on the indexer's own read
//! connection (never one of the turns' readers), applies the text rules of
//! [`text`] on the blocking pool, one row at a time, and writes the page with one
//! writer job (`search_apply`). Rounds go on until the index has every event.
//!
//! When the task wakes:
//!
//! - at the start, at once;
//! - after a committed batch that ends a turn (`turn_completed`, `turn_failed`,
//!   `turn_interrupted`, `turn_cancelled`) or takes text back (`prompt_withdrawn`,
//!   `steering_withdrawn`), and after the broadcast lagged: it first waits
//!   [`DEBOUNCE`] on the clock, so more turn ends in that time join one round.
//!
//! Between rounds it holds no timer: it waits for the broadcast only.
//!
//! A backfill (the first start after the upgrade, a catch-up after a long stop, a new
//! redaction) reads many pages. After each page that leaves more to read, the task
//! waits [`DUTY_FACTOR`] times the time of its writer job, at least [`MIN_PAUSE`], so
//! the backfill takes at most a fifth of the writer. While a turn runs, it writes at
//! most one page in [`TURN_PAGE_INTERVAL`]. After a backfill, it merges the segments of
//! the index ([`MERGE_PAGES`] at a time, at the same duty) until a merge does no work.
//!
//! A text whose rules panic gives no doc: the task logs its sequence number, never its
//! text, and goes on. A store error waits [`BACKOFF_FIRST`], then twice as long each
//! time up to [`BACKOFF_MAX`], with one log line each time; a success ends the backoff.
//! A redaction version that differs from the index's (`REDACTION_VERSION`) clears the
//! index and builds it again from the start of the log.
//!
//! After each change the task publishes a [`SearchIndexStatus`] on a `watch`, which the
//! tests wait on.

mod text;

use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_conversation::REDACTION_VERSION;
use efr_protocol::{Event, Seq, TurnId};
use efr_stdx::time::Clock;
use efr_store::conversations::TurnStatus;
use efr_store::search::{
    self, DocKind, IndexReader, Pending, SearchApplied, SearchPage, SearchState, SourceRow,
};
use efr_store::{Committed, StoreError, WriterHandle};
use jiff::Timestamp;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub use text::index_text;

/// How long the task waits after a turn's end before its round, so the turn ends of
/// that time join one round.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(500);

/// How many times the time of a backfill or merge job the task waits after it.
pub(crate) const DUTY_FACTOR: u32 = 4;

/// The shortest wait after a backfill or merge job. A page of 32 docs takes about 1 ms
/// of the writer, so the duty stays below a fifth, and a backfill of 20,000 turns
/// (7,500 pages) takes about a minute; 10 ms would take more than 90 s.
pub(crate) const MIN_PAUSE: Duration = Duration::from_millis(5);

/// While a turn runs, the backfill writes at most one page or merge in this time.
pub(crate) const TURN_PAGE_INTERVAL: Duration = Duration::from_secs(1);

/// The leaves of the index that one merge job writes, of 1,000 bytes each.
pub(crate) const MERGE_PAGES: u32 = 64;

/// The first wait after a store error.
pub(crate) const BACKOFF_FIRST: Duration = Duration::from_secs(1);

/// The longest wait after store errors in a row.
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// What the indexer has done, published after each change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchIndexStatus {
    /// Every event up to this one is in the index.
    pub indexed_seq: Seq,
    /// The task sleeps on the clock until this time: a debounce, a pause of the duty
    /// cycle or a backoff. `None` while it works or waits for the broadcast.
    pub sleeping_until: Option<Timestamp>,
    /// The pages that the task wrote since it started.
    pub pages: u64,
    /// The merge jobs that the task ran since it started.
    pub merges: u64,
    /// The time of the writer jobs of the pages, as the task waited for them on its
    /// clock.
    pub page_time: Duration,
    /// The time of the writer jobs of the merges, as the task waited for them.
    pub merge_time: Duration,
    /// True while merges after a backfill are due.
    pub merging: bool,
    /// True while the task has nothing to do and waits for the broadcast.
    pub idle: bool,
}

/// The store as the indexer uses it. efrd's is [`StoreIndex`]; a test's can fail.
#[async_trait]
pub(crate) trait IndexStore: Send + Sync + 'static {
    /// The state of the index and, when its redaction is `redaction`, the next page
    /// after its cursor, in one read.
    async fn read(&self, redaction: u32) -> Result<(SearchState, Option<Pending>), StoreError>;
    /// The turns that run now.
    async fn running_turns(&self) -> Result<HashSet<TurnId>, StoreError>;
    /// See `WriterHandle::search_apply`.
    async fn apply(&self, page: SearchPage) -> Result<SearchApplied, StoreError>;
    /// See `WriterHandle::search_reset`.
    async fn reset(&self, redaction: u32) -> Result<(), StoreError>;
    /// See `WriterHandle::search_merge`.
    async fn merge(&self, pages: u32) -> Result<bool, StoreError>;
    /// A receiver of the committed batches.
    fn subscribe(&self) -> broadcast::Receiver<Committed>;
}

/// The store of a running efrd: its writer, and the indexer's own reader.
#[derive(Debug, Clone)]
pub(crate) struct StoreIndex {
    pub(crate) writer: WriterHandle,
    pub(crate) reader: IndexReader,
}

#[async_trait]
impl IndexStore for StoreIndex {
    async fn read(&self, redaction: u32) -> Result<(SearchState, Option<Pending>), StoreError> {
        self.reader
            .with(move |conn| {
                let state = search::state(conn)?;
                if state.redaction != redaction {
                    return Ok((state, None));
                }
                let pending = search::pending(conn, state.indexed_seq)?;
                Ok((state, Some(pending)))
            })
            .await
    }

    async fn running_turns(&self) -> Result<HashSet<TurnId>, StoreError> {
        let turns = self.reader.with(efr_store::conversations::unfinished_turns).await?;
        Ok(turns
            .into_iter()
            .filter(|turn| turn.status == TurnStatus::Running)
            .map(|turn| turn.id)
            .collect())
    }

    async fn apply(&self, page: SearchPage) -> Result<SearchApplied, StoreError> {
        self.writer.search_apply(page).await
    }

    async fn reset(&self, redaction: u32) -> Result<(), StoreError> {
        self.writer.search_reset(redaction).await
    }

    async fn merge(&self, pages: u32) -> Result<bool, StoreError> {
        self.writer.search_merge(pages).await
    }

    fn subscribe(&self) -> broadcast::Receiver<Committed> {
        self.writer.subscribe()
    }
}

/// The text rules of one doc; efrd's is [`index_text`], a test's can panic.
pub(crate) type Rules = Arc<dyn Fn(DocKind, &str) -> Option<String> + Send + Sync>;

/// A new status channel, with nothing indexed yet.
pub(crate) fn status_channel()
-> (watch::Sender<SearchIndexStatus>, watch::Receiver<SearchIndexStatus>) {
    watch::channel(SearchIndexStatus::default())
}

/// Keeps the index of `writer`'s store current until `stop`, reading on `reader`,
/// which must be the indexer's own (one connection, apart from the turns' readers), and
/// publishing on `status`.
pub async fn follow(
    writer: WriterHandle,
    reader: IndexReader,
    clock: Arc<dyn Clock>,
    status: watch::Sender<SearchIndexStatus>,
    stop: CancellationToken,
) {
    let store = StoreIndex { writer, reader };
    Indexer::new(store, clock, Arc::new(index_text), status, stop).run().await;
}

/// The running task.
pub(crate) struct Indexer<S> {
    store: S,
    clock: Arc<dyn Clock>,
    rules: Rules,
    status: watch::Sender<SearchIndexStatus>,
    stop: CancellationToken,
    committed: broadcast::Receiver<Committed>,
    /// The turns that run, from the broadcast.
    running: HashSet<TurnId>,
    /// True when a batch asked for a round that has not run yet.
    woken: bool,
    /// The newest event of a batch that asked for a round.
    wake_seq: Seq,
    /// True when the running turns must be read again, after the broadcast lagged.
    lagged: bool,
    /// When the last page or merge started, and how long its writer job took.
    last_job: Option<(Timestamp, Duration)>,
    /// The wait after the last store error.
    backoff: Option<Duration>,
}

/// Why a step of the task ended early.
enum Halt {
    /// efrd stops.
    Stopped,
    /// The store failed.
    Store(StoreError),
}

impl From<StoreError> for Halt {
    fn from(error: StoreError) -> Self {
        Halt::Store(error)
    }
}

impl<S: IndexStore> Indexer<S> {
    pub(crate) fn new(
        store: S,
        clock: Arc<dyn Clock>,
        rules: Rules,
        status: watch::Sender<SearchIndexStatus>,
        stop: CancellationToken,
    ) -> Self {
        let committed = store.subscribe();
        Indexer {
            store,
            clock,
            rules,
            status,
            stop,
            committed,
            running: HashSet::new(),
            woken: true,
            wake_seq: Seq::ZERO,
            lagged: false,
            last_job: None,
            backoff: None,
        }
    }

    /// Runs until `stop`.
    pub(crate) async fn run(mut self) {
        // NOTE: the start needs no debounce: efrd's reconciliation has ended every turn
        // that ran, so nothing more joins this round.
        let mut debounce = false;
        loop {
            let step = if self.woken {
                if debounce && self.sleep(DEBOUNCE).await.is_err() {
                    return;
                }
                self.woken = false;
                self.catch_up().await
            } else if self.status.borrow().merging {
                self.merge_step().await
            } else {
                if self.wait_for_wake().await.is_err() {
                    return;
                }
                debounce = true;
                continue;
            };
            match step {
                Ok(()) => self.backoff = None,
                Err(Halt::Stopped) => return,
                Err(Halt::Store(error)) => {
                    let wait =
                        self.backoff.map_or(BACKOFF_FIRST, |last| (last * 2).min(BACKOFF_MAX));
                    self.backoff = Some(wait);
                    tracing::warn!(
                        error = %error,
                        retry_secs = wait.as_secs(),
                        "the search index could not be updated; efrd tries again"
                    );
                    if self.sleep(wait).await.is_err() {
                        return;
                    }
                    self.woken = true;
                    debounce = false;
                }
            }
        }
    }

    /// Rounds until the index has every event. While it pauses after a page, the task
    /// already reads and cleans the next one, so a backfill waits for the pause or the
    /// read, not for both.
    async fn catch_up(&mut self) -> Result<(), Halt> {
        let mut pages = 0u32;
        let mut next = None;
        loop {
            self.drain()?;
            if self.lagged {
                self.running = self.store.running_turns().await?;
                self.lagged = false;
            }
            let (state, page) = match next.take() {
                Some(prepared) => prepared,
                None => self.prepare().await?,
            };
            let Some(page) = page else {
                tracing::info!(
                    from = state.redaction,
                    to = REDACTION_VERSION,
                    "the redaction of the search index changed; efrd builds it again"
                );
                self.store.reset(REDACTION_VERSION).await?;
                self.publish(|status| status.indexed_seq = Seq::ZERO);
                continue;
            };
            self.publish(|status| status.indexed_seq = state.indexed_seq);
            if page.through == page.after {
                break;
            }
            let through = page.through;
            let started = self.clock.now();
            let applied = self.store.apply(page).await?;
            let job = self.since(started);
            self.last_job = Some((started, job));
            if applied == SearchApplied::Applied {
                pages += 1;
                self.publish(|status| {
                    status.indexed_seq = through;
                    status.pages += 1;
                    status.page_time += job;
                });
            }
            if through < state.newest_seq {
                let (paused, prepared) = tokio::join!(self.pause(), self.prepare());
                paused?;
                next = Some(prepared?);
            }
        }
        if pages > 1 {
            self.publish(|status| status.merging = true);
        }
        // NOTE: the last read of the round saw every batch that the round took in
        // before it, so only a later batch, or one that the broadcast lost, asks for
        // another round.
        self.woken = self.lagged || self.wake_seq > self.status.borrow().indexed_seq;
        Ok(())
    }

    /// The state of the index and, unless its redaction is another one, the next page
    /// after its cursor with the text rules applied.
    async fn prepare(&self) -> Result<(SearchState, Option<SearchPage>), Halt> {
        let (state, pending) = self.store.read(REDACTION_VERSION).await?;
        let Some(pending) = pending else { return Ok((state, None)) };
        let page = SearchPage {
            after: state.indexed_seq,
            through: pending.through,
            redaction: REDACTION_VERSION,
            docs: self.clean(pending.rows).await?,
            drops: pending.drops,
        };
        Ok((state, Some(page)))
    }

    /// One merge of the segments after a backfill, at the duty cycle.
    async fn merge_step(&mut self) -> Result<(), Halt> {
        self.drain()?;
        if self.woken {
            return Ok(());
        }
        self.pause().await?;
        let started = self.clock.now();
        let worked = self.store.merge(MERGE_PAGES).await?;
        let job = self.since(started);
        self.last_job = Some((started, job));
        self.publish(|status| {
            status.merges += 1;
            status.merge_time += job;
            status.merging = worked;
        });
        Ok(())
    }

    /// Waits for a batch that asks for a round. Updates the running turns on the way.
    async fn wait_for_wake(&mut self) -> Result<(), Halt> {
        self.publish(|status| status.idle = true);
        loop {
            let batch = tokio::select! {
                () = self.stop.cancelled() => return Err(Halt::Stopped),
                batch = self.committed.recv() => batch,
            };
            match batch {
                Ok(batch) => self.see(&batch),
                Err(RecvError::Lagged(_)) => self.lag(),
                Err(RecvError::Closed) => return Err(Halt::Stopped),
            }
            if self.woken {
                self.publish(|status| status.idle = false);
                return Ok(());
            }
        }
    }

    /// Takes in the batches that came since the last look.
    fn drain(&mut self) -> Result<(), Halt> {
        if self.stop.is_cancelled() {
            return Err(Halt::Stopped);
        }
        loop {
            match self.committed.try_recv() {
                Ok(batch) => self.see(&batch),
                Err(TryRecvError::Lagged(_)) => self.lag(),
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Closed) => return Err(Halt::Stopped),
            }
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
                    self.wake(envelope.seq);
                }
                Event::PromptWithdrawn { .. } | Event::SteeringWithdrawn { .. } => {
                    self.wake(envelope.seq);
                }
                _ => {}
            }
        }
    }

    fn wake(&mut self, seq: Seq) {
        self.woken = true;
        self.wake_seq = self.wake_seq.max(seq);
    }

    /// The broadcast lost batches: a round may be due, and the running turns are read
    /// again.
    fn lag(&mut self) {
        self.lagged = true;
        self.woken = true;
    }

    /// The wait before the next page or merge: [`DUTY_FACTOR`] times the last job, at
    /// least [`MIN_PAUSE`]; while a turn runs, until [`TURN_PAGE_INTERVAL`] after the
    /// last job started.
    async fn pause(&self) -> Result<(), Halt> {
        let Some((started, job)) = self.last_job else { return Ok(()) };
        let mut wait = (job * DUTY_FACTOR).max(MIN_PAUSE);
        if !self.running.is_empty() {
            let since = self.since(started);
            wait = wait.max(TURN_PAGE_INTERVAL.saturating_sub(since));
        }
        self.sleep(wait).await
    }

    /// Sleeps `wait` on the clock, publishing when it ends; `Stopped` when efrd stops.
    async fn sleep(&self, wait: Duration) -> Result<(), Halt> {
        // NOTE: the sleep starts before the status says so, so a test that reads the
        // status and moves the clock always ends this sleep.
        let sleep = self.clock.sleep(wait);
        let until = self.clock.now().checked_add(wait).ok();
        self.publish(|status| status.sleeping_until = until);
        let result = tokio::select! {
            () = self.stop.cancelled() => Err(Halt::Stopped),
            () = sleep => Ok(()),
        };
        self.publish(|status| status.sleeping_until = None);
        result
    }

    /// The text rules on each row, on the blocking pool. A row whose rules panic gives
    /// no doc. `Stopped` when efrd stops in the middle, so the page is not written.
    async fn clean(&self, rows: Vec<SourceRow>) -> Result<Vec<SourceRow>, Halt> {
        if rows.is_empty() {
            return Ok(rows);
        }
        let rules = Arc::clone(&self.rules);
        let stop = self.stop.clone();
        let cleaned = tokio::task::spawn_blocking(move || {
            let mut docs = Vec::with_capacity(rows.len());
            for row in rows {
                if stop.is_cancelled() {
                    return None;
                }
                let text = panic::catch_unwind(AssertUnwindSafe(|| rules(row.kind, &row.text)));
                match text {
                    Ok(Some(text)) => docs.push(SourceRow { text, ..row }),
                    Ok(None) => {}
                    Err(_) => tracing::warn!(
                        seq = %row.seq,
                        "a text could not be prepared for the search index; efrd leaves it out"
                    ),
                }
            }
            Some(docs)
        })
        .await;
        match cleaned {
            Ok(Some(docs)) => Ok(docs),
            Ok(None) => Err(Halt::Stopped),
            Err(_) => Err(Halt::Store(StoreError::TaskPanicked { task: "search index rules" })),
        }
    }

    fn since(&self, started: Timestamp) -> Duration {
        Duration::try_from(self.clock.now().duration_since(started)).unwrap_or_default()
    }

    fn publish(&self, change: impl FnOnce(&mut SearchIndexStatus)) {
        self.status.send_if_modified(|status| {
            let before = *status;
            change(status);
            *status != before
        });
    }
}

#[cfg(test)]
mod tests;
