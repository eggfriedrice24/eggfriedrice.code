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
    M::up(include_str!("migrations/0006_compactions.sql")),
    M::up(include_str!("migrations/0007_jobs.sql")),
    M::up(include_str!("migrations/0008_routes.sql")),
    M::up(include_str!("migrations/0009_search.sql")),
    M::up(include_str!("migrations/0010_deletes.sql")),
    M::up(include_str!("migrations/0011_devices.sql")),
];

/// For each step of [`STEPS`], whether it adds or reshapes a projection, so that a
/// database it migrates needs the projections rebuilt from the log. A step that adds
/// only tables apart from the projections, such as the search index, needs none, and
/// the start of efrd after it does not replay the whole log.
pub(crate) const REBUILDS: &[bool] =
    &[true, true, true, true, true, true, true, true, false, false, false];

const _: () = assert!(REBUILDS.len() == STEPS.len(), "each step needs its rebuild flag");

/// The backups hold every conversation, like the database itself.
const BACKUP_DIR_MODE: u32 = 0o700;

/// How many copies `backups/` keeps: the newest by schema version. The copy of the last
/// migration and the one before it let the user go back two schema versions; an older
/// copy only keeps conversations that the user deleted after it was taken.
pub const KEPT_BACKUPS: usize = 2;

/// The store's schema as a sequence of migrations.
#[derive(Debug, Clone)]
pub struct Migrations {
    steps: rusqlite_migration::Migrations<'static>,
    count: usize,
    rebuilds: &'static [bool],
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
    /// True when the projections were rebuilt from the log after the migrations,
    /// because one of them adds or reshapes a projection.
    pub rebuilt: bool,
    /// The older copies of `backups/` that the prune removed, beyond the newest
    /// [`KEPT_BACKUPS`].
    pub pruned: Vec<PathBuf>,
    /// The older copies that the prune could not remove, or the directory when it could
    /// not be read, each with the kind of its error, such as `PermissionDenied`. The
    /// next start tries again.
    pub not_pruned: Vec<(PathBuf, io::ErrorKind)>,
}

impl MigrationReport {
    pub(crate) fn new(
        from: u32,
        to: u32,
        backup: Option<PathBuf>,
        rebuilt: bool,
    ) -> MigrationReport {
        MigrationReport { from, to, backup, rebuilt, pruned: Vec::new(), not_pruned: Vec::new() }
    }

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
        Self::from_steps(STEPS, REBUILDS)
    }

    /// The migrations `steps`, with the rebuild flag of each in `rebuilds`; a step
    /// without a flag rebuilds.
    pub(crate) fn from_steps(steps: &'static [M<'static>], rebuilds: &'static [bool]) -> Self {
        Migrations {
            steps: rusqlite_migration::Migrations::from_slice(steps),
            count: steps.len(),
            rebuilds,
        }
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
    /// projections when the database had a schema before and an applied step adds or
    /// reshapes a projection.
    ///
    /// When the database already has a schema and a migration is pending, a copy of
    /// it is written to `backups/<file name>.<version>` first (with mode 0600, through
    /// `VACUUM INTO`, which also captures what is still in the write-ahead log). A new
    /// database, an in-memory one or a `None` directory gets no copy.
    ///
    /// After the migrations, also when none was pending, the copies of the database's
    /// file beyond the newest [`KEPT_BACKUPS`] go: a copy holds every conversation of its
    /// time, also the ones that the user deleted later. A copy that cannot be removed
    /// stays, and the report names it.
    ///
    /// Fails with [`StoreError::SchemaTooNew`] for a database from a newer build,
    /// which this build must not touch.
    pub fn migrate(
        &self,
        conn: &mut Connection,
        backups: Option<&Path>,
    ) -> Result<MigrationReport, StoreError> {
        let mut report = self.migrate_only(conn, backups)?;
        if let Some(dir) = backups {
            (report.pruned, report.not_pruned) = prune(conn, dir, report.backup.as_deref());
        }
        Ok(report)
    }

    fn migrate_only(
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
            return Ok(MigrationReport::new(from, from, None, false));
        }
        let backup = match backups {
            Some(dir) if from > 0 => backup(conn, dir, from)?,
            _ => None,
        };
        for version in from + 1..=latest {
            // One call per file, so each file commits on its own.
            self.steps.to_version(conn, version as usize)?;
        }
        // A migration may add or reshape a projection, and projections are a function
        // of the log, so they are rebuilt rather than migrated row by row. The steps
        // run from index `from` (version `from + 1`) on.
        let rebuilt = from > 0
            && (from..latest)
                .any(|index| self.rebuilds.get(index as usize).copied().unwrap_or(true));
        if rebuilt {
            let tx = conn.transaction()?;
            projection::rebuild(&tx)?;
            tx.commit()?;
        }
        Ok(MigrationReport::new(from, latest, backup, rebuilt))
    }
}

/// Removes the copies of the database's file in `dir` beyond the newest
/// [`KEPT_BACKUPS`] by schema version, where `taken`, the copy that this start took,
/// counts as the newest: the removed ones, and the ones that could not be removed (or
/// `dir` when it could not be read), each with the kind of its error. Other files stay.
fn prune(
    conn: &Connection,
    dir: &Path,
    taken: Option<&Path>,
) -> (Vec<PathBuf>, Vec<(PathBuf, io::ErrorKind)>) {
    let Some(name) = database_file_name(conn) else {
        return (Vec::new(), Vec::new());
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
        Err(error) => return (Vec::new(), vec![(dir.to_path_buf(), error.kind())]),
    };
    let prefix = format!("{name}.");
    let mut copies: Vec<(u32, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file = entry.file_name();
            let version = file.to_str()?.strip_prefix(&prefix)?;
            let digits = !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit());
            let version = version.parse().ok().filter(|_| digits)?;
            Some((version, entry.path()))
        })
        .collect();
    copies.sort_unstable_by_key(|(version, path)| {
        (Some(path.as_path()) != taken, std::cmp::Reverse(*version))
    });
    let (mut pruned, mut not_pruned) = (Vec::new(), Vec::new());
    for (_, path) in copies.into_iter().skip(KEPT_BACKUPS) {
        match fs::remove_file(&path) {
            Ok(()) => pruned.push(path),
            Err(error) => not_pruned.push((path, error.kind())),
        }
    }
    (pruned, not_pruned)
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
