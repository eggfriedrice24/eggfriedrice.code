//! Read-only access to the store.
//!
//! [`Readers::with`] runs a closure with a connection inside one read transaction, so
//! every query in the closure sees the same snapshot. For a database file the
//! connections are opened read-only (and `query_only`), at most `count` at a time,
//! reused, and used on tokio's blocking pool; in WAL mode they never wait for the
//! writer. A private in-memory database has one connection, the writer's, so an
//! in-memory store's readers run on the writer thread with `query_only` set for the
//! closure.
//!
//! Whether one dedicated reader thread beats this small pool is open question 13 of
//! the structure document; `with` stays the same either way.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;
use tokio::sync::Semaphore;

use crate::{StoreError, WriterHandle, db};

/// The default number of read-only connections.
pub const DEFAULT_READERS: usize = 4;

/// A cheap, cloneable handle for running reads.
#[derive(Debug, Clone)]
pub struct Readers {
    backend: Backend,
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
    permits: Semaphore,
}

impl Readers {
    /// Readers over the database file at `path`, which the writer has opened and
    /// migrated. Up to `count` connections are opened as reads need them.
    pub fn open(path: impl Into<PathBuf>, count: usize) -> Self {
        let count = count.max(1);
        let pool = Pool {
            path: path.into(),
            idle: Mutex::new(Vec::with_capacity(count)),
            permits: Semaphore::new(count),
        };
        Readers { backend: Backend::Pool(Arc::new(pool)) }
    }

    /// Readers that run on the writer's connection, for an in-memory database. They
    /// keep the writer running as long as they exist.
    pub fn through_writer(writer: WriterHandle) -> Self {
        Readers { backend: Backend::Writer(writer) }
    }

    /// Runs `f` with a read-only connection inside one read transaction and returns
    /// its result. The read functions of the table modules, such as
    /// [`events::read_after`](crate::events::read_after), are meant for `f`.
    ///
    /// A write inside `f` fails.
    pub async fn with<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        match &self.backend {
            Backend::Pool(pool) => {
                // The semaphore is never closed, so this always gets a permit; without
                // one the read would still be correct, only unbounded.
                let _permit = pool.permits.acquire().await.ok();
                let pool = Arc::clone(pool);
                tokio::task::spawn_blocking(move || pool.read(f))
                    .await
                    .map_err(|_| StoreError::TaskPanicked { task: "reader" })?
            }
            Backend::Writer(writer) => writer.read(f).await,
        }
    }
}

impl Pool {
    fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
        let conn = match idle {
            Some(conn) => conn,
            None => db::open_read_only(&self.path)?,
        };
        let result = read_in_transaction(&conn, f);
        // A connection whose closure failed is still a good connection.
        self.idle.lock().unwrap_or_else(PoisonError::into_inner).push(conn);
        result
    }
}

/// Runs `f` inside a deferred transaction, whose snapshot starts at the first read.
pub(crate) fn read_in_transaction<T>(
    conn: &Connection,
    f: impl FnOnce(&Connection) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let tx = conn.unchecked_transaction()?;
    let value = f(&tx)?;
    // Nothing was written, so ending the transaction either way is the same.
    tx.commit()?;
    Ok(value)
}

#[cfg(test)]
mod tests;
