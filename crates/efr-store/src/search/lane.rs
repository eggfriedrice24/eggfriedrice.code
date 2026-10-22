//! The read connections of search, apart from the readers of the turns (the plan's
//! DQ19).
//!
//! [`SearchLanes`] are [`SEARCH_LANES`] read-only connections with a semaphore of their
//! own: the list of a terminal, its preview and a phone can search at once, and a burst
//! of keys never takes one of the [`Readers`](crate::Readers) that `prompt.send` and the
//! turns use. The permit of a lane moves into the blocking work and goes back only when
//! that work ends, so a search whose caller stopped waiting still holds its lane until
//! the [`SearchStop`] ends it. [`IndexReader`] is the one read-only connection of efrd's
//! indexer, outside the semaphore, so the backfill neither takes a turn reader nor
//! waits for a search.
//!
//! Each connection sets a page cache of [`SEARCH_CACHE_KIB`] KiB. That is no hard cap:
//! the bundled SQLite (built with `SQLITE_ENABLE_MEMORY_MANAGEMENT`) puts the page
//! caches of all connections of the process in one group, whose limit is the sum of
//! their sizes, so a lane can also use the room that the writer and the readers leave.
//! An in-memory store has one connection, the writer's, so its
//! lanes and its index reader run on the writer thread, with the stop's flags but no
//! interrupt.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_stdx::time::Clock;
use rusqlite::Connection;
use tokio::sync::Semaphore;

use crate::reader::read_in_transaction;
use crate::search::SearchStop;
use crate::{StoreError, WriterHandle, db};

/// The searches that run at once; another one waits for a lane.
pub const SEARCH_LANES: usize = 3;

/// The page cache of each search connection, in KiB.
pub const SEARCH_CACHE_KIB: i64 = 512;

/// The read connections of search, behind a semaphore of their own. Clones share them.
#[derive(Debug, Clone)]
pub struct SearchLanes {
    backend: Backend,
    permits: Arc<Semaphore>,
    clock: Arc<dyn Clock>,
}

#[derive(Debug, Clone)]
enum Backend {
    Pool(Arc<Pool>),
    Writer(WriterHandle),
}

#[derive(Debug)]
struct Pool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

impl SearchLanes {
    /// Lanes over the database file at `path`, which the writer has opened and
    /// migrated. A connection opens when a search first needs it. `clock` times the
    /// wait for a lane.
    pub fn open(path: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        let pool = Pool { path: path.into(), idle: Mutex::new(Vec::with_capacity(SEARCH_LANES)) };
        SearchLanes {
            backend: Backend::Pool(Arc::new(pool)),
            permits: Arc::new(Semaphore::new(SEARCH_LANES)),
            clock,
        }
    }

    /// Lanes that run on the writer's connection, for an in-memory database. They keep
    /// the writer running as long as they exist.
    pub fn through_writer(writer: WriterHandle, clock: Arc<dyn Clock>) -> Self {
        SearchLanes {
            backend: Backend::Writer(writer),
            permits: Arc::new(Semaphore::new(SEARCH_LANES)),
            clock,
        }
    }

    /// Runs `f` on a lane inside one read transaction and returns its result.
    ///
    /// Waits at most `wait` on the clock for a lane, then fails with
    /// [`StoreError::SearchBusy`]. The permit lives in the blocking work: when the
    /// caller stops waiting, the lane stays taken until `f` ends. While `f` runs, the
    /// lane's interrupt is in `stop`; after [`SearchStop::hard`], an error of `f` is
    /// [`StoreError::SearchStopped`], and a search that has not started does not start.
    pub async fn with<T, F>(&self, stop: &SearchStop, wait: Duration, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        if stop.is_hard() {
            return Err(StoreError::SearchStopped);
        }
        let permit = match self.clock.timeout(wait, Arc::clone(&self.permits).acquire_owned()).await
        {
            Ok(Ok(permit)) => permit,
            // The semaphore is never closed; a closed one would be no lane either.
            Ok(Err(_)) | Err(_) => return Err(StoreError::SearchBusy),
        };
        let stop = stop.clone();
        match &self.backend {
            Backend::Pool(pool) => {
                let pool = Arc::clone(pool);
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    efr_stdx::panic::quiet(|| pool.search(&stop, f))
                })
                .await
                .map_err(|_| StoreError::TaskPanicked { task: "search lane" })?
            }
            Backend::Writer(writer) => {
                writer
                    .read(move |conn| {
                        let _permit = permit;
                        if stop.is_hard() {
                            return Err(StoreError::SearchStopped);
                        }
                        efr_stdx::panic::quiet(|| f(conn)).map_err(|error| {
                            if stop.is_hard() { StoreError::SearchStopped } else { error }
                        })
                    })
                    .await
            }
        }
    }
}

impl Pool {
    fn search<T>(
        &self,
        stop: &SearchStop,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
        let conn = match idle {
            Some(conn) => conn,
            None => open_search_connection(&self.path)?,
        };
        let result = if stop.arm(conn.get_interrupt_handle()) {
            let result = read_in_transaction(&conn, f);
            // NOTE: the handle leaves the stop before the connection goes back, so a
            // late stop cannot interrupt the next search on this lane.
            stop.disarm();
            result.map_err(|error| if stop.is_hard() { StoreError::SearchStopped } else { error })
        } else {
            Err(StoreError::SearchStopped)
        };
        // An interrupted connection serves the next search: SQLite clears the
        // interrupt when the next statement starts with no other one running.
        self.idle.lock().unwrap_or_else(PoisonError::into_inner).push(conn);
        result
    }
}

/// A read-only connection for search, with the small page cache and the vocabulary of
/// the index ([`VOCABULARY`]).
fn open_search_connection(path: &Path) -> Result<Connection, StoreError> {
    let conn = db::open_read_only(path)?;
    conn.pragma_update(None, "cache_size", -SEARCH_CACHE_KIB)
        .map_err(|source| StoreError::Configure { pragma: "cache_size", source })?;
    // NOTE: the vocabulary is a table of the connection's own temp schema, which
    // stores nothing: it reads the index of the file. `query_only` refuses even that
    // schema change, so it is off for this one statement.
    conn.pragma_update(None, "query_only", false)
        .map_err(|source| StoreError::Configure { pragma: "query_only", source })?;
    let created = conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE temp.{VOCABULARY} USING fts5vocab(main, search_fts, row);"
    ));
    conn.pragma_update(None, "query_only", true)
        .map_err(|source| StoreError::Configure { pragma: "query_only", source })?;
    created?;
    Ok(conn)
}

/// The `fts5vocab` table over the index that each search connection has in its temp
/// schema: the words of the index in order, so a search can expand a prefix that no
/// prefix index serves to the words that start with it. An in-memory store has none.
pub(crate) const VOCABULARY: &str = "search_vocab";

/// The read connection of efrd's indexer: one connection, outside the lanes and the
/// readers. Clones share it; reads run one at a time.
#[derive(Debug, Clone)]
pub struct IndexReader {
    backend: IndexBackend,
}

#[derive(Debug, Clone)]
enum IndexBackend {
    File(Arc<IndexConnection>),
    Writer(WriterHandle),
}

#[derive(Debug)]
struct IndexConnection {
    path: PathBuf,
    conn: Mutex<Option<Connection>>,
}

impl IndexReader {
    /// The index reader of the database file at `path`. Its connection opens on the
    /// first read.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let file = IndexConnection { path: path.into(), conn: Mutex::new(None) };
        IndexReader { backend: IndexBackend::File(Arc::new(file)) }
    }

    /// An index reader on the writer's connection, for an in-memory database.
    pub fn through_writer(writer: WriterHandle) -> Self {
        IndexReader { backend: IndexBackend::Writer(writer) }
    }

    /// Runs `f` with the indexer's connection inside one read transaction, such as
    /// [`search::state`](crate::search::state) and
    /// [`search::pending`](crate::search::pending).
    pub async fn with<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        match &self.backend {
            IndexBackend::File(file) => {
                let file = Arc::clone(file);
                tokio::task::spawn_blocking(move || efr_stdx::panic::quiet(|| file.read(f)))
                    .await
                    .map_err(|_| StoreError::TaskPanicked { task: "index reader" })?
            }
            IndexBackend::Writer(writer) => {
                writer.read(move |conn| efr_stdx::panic::quiet(|| f(conn))).await
            }
        }
    }
}

impl IndexConnection {
    fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        let conn = match slot.take() {
            Some(conn) => conn,
            None => open_search_connection(&self.path)?,
        };
        let result = read_in_transaction(&conn, f);
        *slot = Some(conn);
        result
    }
}

#[cfg(test)]
mod tests;
