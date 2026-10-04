//! An in-memory efr store for tests.

use std::sync::Arc;

use efr_protocol::{EventEnvelope, Seq};
use efr_stdx::time::Clock;
use efr_store::recording::Recordings;
use efr_store::{Readers, Store, WriterHandle, events};
use tempfile::TempDir;

use crate::TestSupportError;

/// The real `efr-store` over a new private in-memory database: the same migrations,
/// writer and read functions as the daemon's, with times from the clock the test
/// passes in. PTY recordings, which are files, go to a temporary directory that is
/// removed with the value.
#[derive(Debug)]
pub struct TestStore {
    store: Store,
    recordings: Recordings,
    // NOTE: held for its Drop, which removes the recording files.
    _recordings_dir: TempDir,
}

impl TestStore {
    /// Opens and migrates the database and starts its writer. Event times come from
    /// `clock`, normally [`TestClock::shared`](crate::TestClock::shared).
    pub async fn open(clock: Arc<dyn Clock>) -> Result<Self, TestSupportError> {
        let recordings_dir = tempfile::Builder::new()
            .prefix("efr-recordings-")
            .tempdir()
            .map_err(|source| TestSupportError::CreateTempDir { source })?;
        let store = Store::open_in_memory(Arc::clone(&clock))
            .await
            .map_err(|source| TestSupportError::OpenStore { source })?;
        let recordings = Recordings::new(
            recordings_dir.path(),
            store.writer().clone(),
            store.readers().clone(),
            clock,
        );
        Ok(TestStore { store, recordings, _recordings_dir: recordings_dir })
    }

    /// The store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The writer.
    pub fn writer(&self) -> &WriterHandle {
        self.store.writer()
    }

    /// The readers. They run on the writer's connection, because an in-memory database
    /// has only one.
    pub fn readers(&self) -> &Readers {
        self.store.readers()
    }

    /// The PTY recordings, in the temporary directory.
    pub fn recordings(&self) -> &Recordings {
        &self.recordings
    }

    /// Every event in the log, in sequence order.
    pub async fn events(&self) -> Result<Vec<EventEnvelope>, TestSupportError> {
        self.readers()
            .with(|conn| events::read_after(conn, Seq::ZERO, u32::MAX))
            .await
            .map_err(|source| TestSupportError::ReadStore { source })
    }

    /// Closes the store and waits until its writer has stopped. Every clone of the
    /// writer, the readers and the recordings that the test made must be dropped first,
    /// or this waits for them.
    pub async fn close(self) {
        let TestStore { store, recordings, _recordings_dir } = self;
        drop(recordings);
        store.close().await;
    }
}

#[cfg(test)]
mod tests;
