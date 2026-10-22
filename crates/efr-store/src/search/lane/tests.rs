use std::future::ready;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;

use efr_stdx::time::Sleep;
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc, watch};

use super::*;
use crate::testing::{self, TestClock};
use crate::writer::DEFAULT_BROADCAST_CAPACITY;
use crate::{Migrations, StoreWriter};

/// A clock whose sleeps end only when the test fires it, and that counts the sleeps
/// that it was asked for.
#[derive(Debug)]
struct GateClock {
    fired: watch::Sender<bool>,
    asked: AtomicUsize,
    asked_notify: Notify,
}

impl GateClock {
    fn new() -> Arc<Self> {
        Arc::new(GateClock {
            fired: watch::channel(false).0,
            asked: AtomicUsize::new(0),
            asked_notify: Notify::new(),
        })
    }

    fn fire(&self) {
        self.fired.send_replace(true);
    }

    /// Waits until `n` sleeps were asked for.
    async fn asked(&self, n: usize) {
        loop {
            let notified = self.asked_notify.notified();
            if self.asked.load(Ordering::SeqCst) >= n {
                return;
            }
            notified.await;
        }
    }
}

impl Clock for GateClock {
    fn now(&self) -> Timestamp {
        testing::start()
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        let mut fired = self.fired.subscribe();
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.asked_notify.notify_waiters();
        Box::pin(async move {
            let _ = fired.wait_for(|fired| *fired).await;
        })
    }
}

/// A clock whose sleeps end at once: a search that finds no free lane is busy.
#[derive(Debug)]
struct NoWait;

impl Clock for NoWait {
    fn now(&self) -> Timestamp {
        testing::start()
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(ready(()))
    }
}

const WAIT: Duration = Duration::from_millis(200);

/// A migrated database file in `dir`, with its writer.
fn file_store(dir: &Path) -> (PathBuf, WriterHandle, StoreWriter) {
    let path = dir.join("efr.sqlite");
    let mut conn = db::open(&path).unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    let (writer, thread) =
        StoreWriter::spawn(conn, TestClock::new(), DEFAULT_BROADCAST_CAPACITY).unwrap();
    (path, writer, thread)
}

/// A statement whose first row comes at once and whose next row takes very long: it
/// counts to a trillion. So a test can stop it while it surely runs.
const SLOW: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) \
                    SELECT x FROM c WHERE x = 1 OR x = 1000000000000";

/// The same shape with an end: its second row comes after a short count.
const SHORT: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 1000) \
                     SELECT x FROM c WHERE x = 1 OR x = 1000";

/// A search that reads the first row of `sql`, says so on `started`, waits for `go`
/// (when given), then reads the rest; it answers the rows it read.
fn stepping(
    sql: &'static str,
    started: mpsc::UnboundedSender<()>,
    go: Option<std_mpsc::Receiver<()>>,
) -> impl FnOnce(&Connection) -> Result<Vec<i64>, StoreError> + Send + 'static {
    move |conn| {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let mut seen = Vec::new();
        if let Some(row) = rows.next()? {
            seen.push(row.get(0)?);
        }
        let _ = started.send(());
        if let Some(go) = go {
            let _ = go.recv();
        }
        while let Some(row) = rows.next()? {
            seen.push(row.get(0)?);
        }
        Ok(seen)
    }
}

/// A search that holds its lane until `release` sends or closes.
fn holding(
    started: mpsc::UnboundedSender<()>,
    release: std_mpsc::Receiver<()>,
) -> impl FnOnce(&Connection) -> Result<(), StoreError> + Send + 'static {
    move |conn| {
        conn.query_row("SELECT 1", [], |_| Ok(()))?;
        let _ = started.send(());
        let _ = release.recv();
        Ok(())
    }
}

fn idle(lanes: &SearchLanes) -> usize {
    let Backend::Pool(pool) = &lanes.backend else { panic!("not a pool") };
    pool.idle.lock().unwrap().len()
}

#[tokio::test]
async fn fourth_search_waits_then_gets_busy() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer, _thread) = file_store(dir.path());
    let clock = GateClock::new();
    let lanes = SearchLanes::open(&path, clock.clone());
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut releases = Vec::new();
    let mut held = Vec::new();
    for _ in 0..SEARCH_LANES {
        let (release_tx, release) = std_mpsc::channel();
        releases.push(release_tx);
        let lanes = lanes.clone();
        let search = holding(started_tx.clone(), release);
        held.push(tokio::spawn(async move { lanes.with(&SearchStop::new(), WAIT, search).await }));
    }
    for _ in 0..SEARCH_LANES {
        started.recv().await.unwrap();
    }

    // A fourth search waits for a lane, and gets one when a lane frees in time.
    let asked = clock.asked.load(Ordering::SeqCst);
    let fourth = {
        let lanes = lanes.clone();
        tokio::spawn(async move {
            lanes
                .with(&SearchStop::new(), WAIT, |conn| {
                    Ok(conn.query_row("SELECT 4", [], |row| row.get::<_, i64>(0))?)
                })
                .await
        })
    };
    clock.asked(asked + 1).await;
    releases.pop().unwrap().send(()).unwrap();
    assert_eq!(fourth.await.unwrap().unwrap(), 4);

    // A fifth one, while every lane is busy again, waits until its time ends.
    let (release_tx, release) = std_mpsc::channel();
    releases.push(release_tx);
    let again = {
        let lanes = lanes.clone();
        let search = holding(started_tx.clone(), release);
        tokio::spawn(async move { lanes.with(&SearchStop::new(), WAIT, search).await })
    };
    started.recv().await.unwrap();
    let asked = clock.asked.load(Ordering::SeqCst);
    let fifth = {
        let lanes = lanes.clone();
        tokio::spawn(async move { lanes.with(&SearchStop::new(), WAIT, |_| Ok(5)).await })
    };
    clock.asked(asked + 1).await;
    clock.fire();
    assert!(matches!(fifth.await.unwrap(), Err(StoreError::SearchBusy)));

    drop(releases);
    for search in held.into_iter().take(SEARCH_LANES - 1) {
        search.await.unwrap().unwrap();
    }
    again.await.unwrap().unwrap();
}

#[tokio::test]
async fn lane_holds_its_permit_until_the_work_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer, _thread) = file_store(dir.path());
    let lanes = SearchLanes::open(&path, Arc::new(NoWait));
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut releases = Vec::new();
    let mut held = Vec::new();
    for _ in 0..SEARCH_LANES {
        let (release_tx, release) = std_mpsc::channel();
        releases.push(release_tx);
        let lanes = lanes.clone();
        let search = holding(started_tx.clone(), release);
        held.push(tokio::spawn(async move { lanes.with(&SearchStop::new(), WAIT, search).await }));
    }
    for _ in 0..SEARCH_LANES {
        started.recv().await.unwrap();
    }

    // The caller of one search stops waiting; its work still runs, so its lane stays
    // taken.
    let dropped = held.pop().unwrap();
    dropped.abort();
    assert!(dropped.await.unwrap_err().is_cancelled());
    let busy = lanes.with(&SearchStop::new(), WAIT, |_| Ok(())).await;
    assert!(matches!(busy, Err(StoreError::SearchBusy)));

    // Its work ends, and its lane frees: a search that waits with no time limit gets
    // it.
    let last = releases.pop().unwrap();
    let waiting = {
        let lanes = SearchLanes { clock: GateClock::new(), ..lanes.clone() };
        tokio::spawn(async move { lanes.with(&SearchStop::new(), WAIT, |_| Ok(())).await })
    };
    last.send(()).unwrap();
    waiting.await.unwrap().unwrap();
    drop(releases);
    for search in held {
        search.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn hard_stop_interrupts_a_slow_vdbe_statement() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer, _thread) = file_store(dir.path());
    let lanes = SearchLanes::open(&path, Arc::new(NoWait));
    let stop = SearchStop::new();
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let search = {
        let lanes = lanes.clone();
        let stop = stop.clone();
        tokio::spawn(async move { lanes.with(&stop, WAIT, stepping(SLOW, started_tx, None)).await })
    };
    started.recv().await.unwrap();
    stop.hard();
    assert!(matches!(search.await.unwrap(), Err(StoreError::SearchStopped)));
    assert_eq!(idle(&lanes), 1, "the interrupted connection went back to the lane");

    // lane_serves_after_an_interrupt: the same connection answers the next search.
    let (started_tx, _started) = mpsc::unbounded_channel();
    let rows =
        lanes.with(&SearchStop::new(), WAIT, stepping(SHORT, started_tx, None)).await.unwrap();
    assert_eq!(rows, vec![1, 1000]);
    assert_eq!(idle(&lanes), 1);
}

#[tokio::test]
async fn late_stop_does_not_interrupt_the_next_search() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer, _thread) = file_store(dir.path());
    let lanes = SearchLanes::open(&path, Arc::new(NoWait));
    let first = SearchStop::new();
    lanes.with(&first, WAIT, |conn| Ok(conn.query_row("SELECT 1", [], |_| Ok(()))?)).await.unwrap();

    let (started_tx, mut started) = mpsc::unbounded_channel();
    let (go_tx, go) = std_mpsc::channel();
    let second = {
        let lanes = lanes.clone();
        tokio::spawn(async move {
            lanes.with(&SearchStop::new(), WAIT, stepping(SHORT, started_tx, Some(go))).await
        })
    };
    started.recv().await.unwrap();
    first.hard();
    go_tx.send(()).unwrap();
    assert_eq!(second.await.unwrap().unwrap(), vec![1, 1000]);
    assert_eq!(idle(&lanes), 1, "both searches ran on the one connection");
}

#[tokio::test]
async fn a_hard_stop_before_the_start_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer, _thread) = file_store(dir.path());
    let lanes = SearchLanes::open(&path, Arc::new(NoWait));
    let stop = SearchStop::new();
    stop.hard();
    let result =
        lanes.with(&stop, WAIT, |_| -> Result<(), StoreError> { panic!("must not run") }).await;
    assert!(matches!(result, Err(StoreError::SearchStopped)));
}

#[tokio::test]
async fn in_memory_store_never_interrupts() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let lanes = SearchLanes::through_writer(writer.clone(), Arc::new(NoWait));
    let stop = SearchStop::new();
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let (go_tx, go) = std_mpsc::channel();
    let search = {
        let lanes = lanes.clone();
        let stop = stop.clone();
        tokio::spawn(
            async move { lanes.with(&stop, WAIT, stepping(SHORT, started_tx, Some(go))).await },
        )
    };
    started.recv().await.unwrap();
    stop.hard();
    go_tx.send(()).unwrap();
    assert_eq!(search.await.unwrap().unwrap(), vec![1, 1000], "the flags only, no interrupt");
    assert!(stop.is_hard() && stop.is_stopped());
}

#[tokio::test]
async fn lanes_set_a_small_page_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer, _thread) = file_store(dir.path());
    let lanes = SearchLanes::open(&path, Arc::new(NoWait));
    let cache: i64 = lanes
        .with(&SearchStop::new(), WAIT, |conn| {
            Ok(conn.pragma_query_value(None, "cache_size", |row| row.get(0))?)
        })
        .await
        .unwrap();
    assert_eq!(cache, -SEARCH_CACHE_KIB);
    let reader = IndexReader::open(&path);
    let cache: i64 = reader
        .with(|conn| Ok(conn.pragma_query_value(None, "cache_size", |row| row.get(0))?))
        .await
        .unwrap();
    assert_eq!(cache, -SEARCH_CACHE_KIB);
    let write = lanes
        .with(&SearchStop::new(), WAIT, |conn| Ok(conn.execute("DELETE FROM search_docs", [])?))
        .await;
    assert!(write.is_err(), "a lane is read-only");
}

#[tokio::test]
async fn index_reader_reads_beside_busy_lanes() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _writer, _thread) = file_store(dir.path());
    let lanes = SearchLanes::open(&path, Arc::new(NoWait));
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let (release_tx, release) = std_mpsc::channel::<()>();
    let release = Arc::new(Mutex::new(release));
    let mut held = Vec::new();
    for _ in 0..SEARCH_LANES {
        let lanes = lanes.clone();
        let started = started_tx.clone();
        let release = Arc::clone(&release);
        held.push(tokio::spawn(async move {
            lanes
                .with(&SearchStop::new(), WAIT, move |conn| {
                    conn.query_row("SELECT 1", [], |_| Ok(()))?;
                    let _ = started.send(());
                    let _ = release.lock().unwrap().recv();
                    Ok(())
                })
                .await
        }));
    }
    for _ in 0..SEARCH_LANES {
        started.recv().await.unwrap();
    }
    let reader = IndexReader::open(&path);
    let state = reader.with(crate::search::state).await.unwrap();
    assert_eq!(state.indexed_seq.get(), 0);
    drop(release_tx);
    for search in held {
        search.await.unwrap().unwrap();
    }
}
