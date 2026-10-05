//! Opening the whole store in one call: the file, the backup, the migrations, the
//! writer and the readers, in the order the daemon's startup needs.

use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_stdx::time::Clock;

use crate::reader::DEFAULT_READERS;
use crate::writer::DEFAULT_BROADCAST_CAPACITY;
use crate::{MigrationReport, Migrations, Readers, StoreError, StoreWriter, WriterHandle, db};

/// The database's file name in the data directory.
pub const DATABASE_FILE: &str = efr_stdx::paths::DATABASE_FILE;

/// The directory, in the data directory, of the copies taken before migrations.
pub const BACKUP_DIR: &str = "backups";

/// The data directory holds every conversation, so only its owner may enter it.
const DATA_DIR_MODE: u32 = 0o700;

/// Where the store lives and how it runs. The daemon builds it from its config and
/// passes it by value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoreConfig {
    /// The database file.
    pub path: PathBuf,
    /// The directory for the copies taken before migrations; `None` takes none.
    pub backups: Option<PathBuf>,
    /// The most read-only connections open at once.
    pub readers: usize,
    /// How many committed batches a subscriber may fall behind by.
    pub broadcast_capacity: usize,
}

impl StoreConfig {
    /// The layout in the data directory (`$XDG_DATA_HOME/efr`): `efr.sqlite` and
    /// `backups/`, with the default reader count and broadcast capacity.
    pub fn in_data_dir(data: &Path) -> Self {
        StoreConfig {
            path: data.join(DATABASE_FILE),
            backups: Some(data.join(BACKUP_DIR)),
            readers: DEFAULT_READERS,
            broadcast_capacity: DEFAULT_BROADCAST_CAPACITY,
        }
    }
}

/// An open, migrated store with its writer running.
///
/// Clone [`Store::writer`] and [`Store::readers`] into the actors that need them;
/// [`Store::close`] waits until every clone is gone and the writer has closed the
/// database.
#[derive(Debug)]
pub struct Store {
    writer: WriterHandle,
    readers: Readers,
    thread: StoreWriter,
    migration: MigrationReport,
}

impl Store {
    /// Opens the database file of `config`, creating it and its directory when they do
    /// not exist, backs it up and migrates it, and starts the writer and the readers.
    pub async fn open(config: StoreConfig, clock: Arc<dyn Clock>) -> Result<Store, StoreError> {
        let path = config.path.clone();
        let backups = config.backups.clone();
        let (conn, migration) = tokio::task::spawn_blocking(move || {
            create_parent(&path)?;
            let mut conn = db::open(&path)?;
            let migration = Migrations::new().migrate(&mut conn, backups.as_deref())?;
            Ok::<_, StoreError>((conn, migration))
        })
        .await
        .map_err(|_| StoreError::TaskPanicked { task: "open" })??;
        let (writer, thread) = StoreWriter::spawn(conn, clock, config.broadcast_capacity)?;
        let readers = Readers::open(config.path, config.readers);
        Ok(Store { writer, readers, thread, migration })
    }

    /// A store in a new private in-memory database, for tests: the same schema and
    /// writer, with reads running on the writer's connection.
    pub async fn open_in_memory(clock: Arc<dyn Clock>) -> Result<Store, StoreError> {
        let (conn, migration) = tokio::task::spawn_blocking(|| {
            let mut conn = db::open_in_memory()?;
            let migration = Migrations::new().migrate(&mut conn, None)?;
            Ok::<_, StoreError>((conn, migration))
        })
        .await
        .map_err(|_| StoreError::TaskPanicked { task: "open" })??;
        let (writer, thread) = StoreWriter::spawn(conn, clock, DEFAULT_BROADCAST_CAPACITY)?;
        let readers = Readers::through_writer(writer.clone());
        Ok(Store { writer, readers, thread, migration })
    }

    /// The writer.
    pub fn writer(&self) -> &WriterHandle {
        &self.writer
    }

    /// The readers.
    pub fn readers(&self) -> &Readers {
        &self.readers
    }

    /// What the migrations did when the store opened.
    pub fn migration(&self) -> &MigrationReport {
        &self.migration
    }

    /// Drops this store's handles and waits until the writer has stopped, which is
    /// when every other clone of the writer and of the readers is gone too.
    pub async fn close(self) {
        let Store { writer, readers, thread, migration: _ } = self;
        drop(readers);
        drop(writer);
        thread.join().await;
    }
}

fn create_parent(path: &Path) -> Result<(), StoreError> {
    let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) else {
        return Ok(());
    };
    DirBuilder::new()
        .recursive(true)
        .mode(DATA_DIR_MODE)
        .create(parent)
        .map_err(|source| StoreError::CreateDir { path: parent.to_path_buf(), source })
}

#[cfg(test)]
mod tests;
