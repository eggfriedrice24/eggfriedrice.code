//! The schema: forward-only SQL migrations and the copy taken before they run.
//!
//! The files in `migrations/` are embedded with `include_str!` and applied by
//! `rusqlite_migration`, which tracks the version in `PRAGMA user_version`. Each file
//! runs in its own transaction, so a failure leaves the database at the last version
//! that succeeded. A shipped file is never edited or renumbered; a change is a new
//! file, and a new column uses `ALTER TABLE ... ADD COLUMN` with a default.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite_migration::M;

use crate::{StoreError, projection};

/// The migrations in order; the position plus one is the version a file produces.
pub(crate) const STEPS: &[M<'static>] = &[
    M::up(include_str!("migrations/0001_events.sql")),
    M::up(include_str!("migrations/0002_conversations.sql")),
    M::up(include_str!("migrations/0003_receipts_outbox.sql")),
    M::up(include_str!("migrations/0004_shells_recordings.sql")),
    M::up(include_str!("migrations/0005_turn_messages.sql")),
];

/// The backups hold every conversation, like the database itself.
const BACKUP_DIR_MODE: u32 = 0o700;

/// The store's schema as a sequence of migrations.
#[derive(Debug, Clone)]
pub struct Migrations {
    steps: rusqlite_migration::Migrations<'static>,
    count: usize,
}

/// What [`Migrations::migrate`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MigrationReport {
    /// The schema version before.
    pub from: u32,
    /// The schema version after.
    pub to: u32,
    /// The copy of the database taken before the first migration ran, when one was
    /// taken.
    pub backup: Option<PathBuf>,
}

impl MigrationReport {
    /// True when at least one migration ran.
    pub fn applied(&self) -> bool {
        self.to > self.from
    }
}

impl Default for Migrations {
    fn default() -> Self {
        Self::new()
    }
}

impl Migrations {
    /// The migrations of this build.
    pub fn new() -> Self {
        Self::from_steps(STEPS)
    }

    pub(crate) fn from_steps(steps: &'static [M<'static>]) -> Self {
        Migrations { steps: rusqlite_migration::Migrations::from_slice(steps), count: steps.len() }
    }

    /// The schema version that [`Migrations::migrate`] brings a database to.
    pub fn latest(&self) -> u32 {
        u32::try_from(self.count).unwrap_or(u32::MAX)
    }

    /// Applies every migration to a new in-memory database, which proves that the SQL
    /// is valid and that the files run in order.
    pub fn validate(&self) -> Result<(), StoreError> {
        self.steps.validate()?;
        Ok(())
    }

    /// The schema version of the database behind `conn`: 0 for a new database.
    pub fn version(conn: &Connection) -> Result<u32, StoreError> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        u32::try_from(version).map_err(|source| StoreError::DecodeColumn {
            table: "pragma",
            column: "user_version",
            source: source.into(),
        })
    }

    /// Brings the database behind `conn` to [`Migrations::latest`], and rebuilds the
    /// projections when the database had a schema before.
    ///
    /// When the database already has a schema and a migration is pending, a copy of
    /// it is written to `backups/<file name>.<version>` first (with mode 0600, through
    /// `VACUUM INTO`, which also captures what is still in the write-ahead log). A new
    /// database, an in-memory one or a `None` directory gets no copy.
    ///
    /// Fails with [`StoreError::SchemaTooNew`] for a database from a newer build,
    /// which this build must not touch.
    pub fn migrate(
        &self,
        conn: &mut Connection,
        backups: Option<&Path>,
    ) -> Result<MigrationReport, StoreError> {
        let from = Self::version(conn)?;
        let latest = self.latest();
        if from > latest {
            return Err(StoreError::SchemaTooNew { found: from, supported: latest });
        }
        if from == latest {
            return Ok(MigrationReport { from, to: from, backup: None });
        }
        let backup = match backups {
            Some(dir) if from > 0 => backup(conn, dir, from)?,
            _ => None,
        };
        for version in from + 1..=latest {
            // One call per file, so each file commits on its own.
            self.steps.to_version(conn, version as usize)?;
        }
        if from > 0 {
            // A migration may add or reshape a projection, and projections are a
            // function of the log, so they are rebuilt rather than migrated row by row.
            let tx = conn.transaction()?;
            projection::rebuild(&tx)?;
            tx.commit()?;
        }
        Ok(MigrationReport { from, to: latest, backup })
    }
}

/// Copies the database to `dir/<file name>.<version>` and returns the path, or `None`
/// for a database without a file.
fn backup(conn: &Connection, dir: &Path, version: u32) -> Result<Option<PathBuf>, StoreError> {
    let Some(name) = database_file_name(conn) else {
        return Ok(None);
    };
    DirBuilder::new()
        .recursive(true)
        .mode(BACKUP_DIR_MODE)
        .create(dir)
        .map_err(|source| StoreError::CreateDir { path: dir.to_path_buf(), source })?;
    let target = dir.join(format!("{name}.{version}"));
    // `VACUUM INTO` refuses a file with content, so the copy goes to a fresh, empty,
    // private temporary file that is renamed over any older copy only when complete.
    let temp = dir.join(format!(".{name}.{version}.tmp"));
    remove_if_present(&temp)?;
    efr_stdx::fs::create_private(&temp)
        .map_err(|source| StoreError::CreateFile { path: temp.clone(), source })?;
    let Some(temp_text) = temp.to_str() else {
        let _ = fs::remove_file(&temp);
        return Err(StoreError::NonUtf8Path { path: temp });
    };
    if let Err(source) = conn.execute("VACUUM INTO ?1", [temp_text]) {
        // The failure is the error to report; a leftover temporary file is harmless.
        let _ = fs::remove_file(&temp);
        return Err(StoreError::Backup { path: target, source });
    }
    fs::rename(&temp, &target)
        .map_err(|source| StoreError::BackupFile { path: target.clone(), source })?;
    Ok(Some(target))
}

/// The file name of the main database, or `None` for an in-memory database.
fn database_file_name(conn: &Connection) -> Option<String> {
    let path = conn.path().filter(|path| !path.is_empty())?;
    Path::new(path).file_name()?.to_str().map(str::to_owned)
}

fn remove_if_present(path: &Path) -> Result<(), StoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StoreError::BackupFile { path: path.to_path_buf(), source }),
    }
}

#[cfg(test)]
mod tests;
