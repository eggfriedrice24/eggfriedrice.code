//! Opening SQLite connections.
//!
//! Every connection in efr comes from this module. `clippy.toml` denies
//! `rusqlite::Connection::open` and `open_in_memory` everywhere else and names
//! [`open`] and [`open_in_memory`] as the replacements, so every connection gets the
//! same pragmas and only this crate decides how the database is opened.
//!
//! These functions block. Async code calls them inside `spawn_blocking`.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_stdx::StdxError;
use rusqlite::{Connection, OpenFlags};

use crate::StoreError;

/// How long a statement waits for a lock before it fails with
/// [`StoreError::Busy`]. Readers in WAL mode never block the writer, so only a
/// checkpoint or a second process can make anyone wait.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// The size the write-ahead log is truncated to after a checkpoint, in bytes.
pub const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

/// Opens the database at `path` for reading and writing, creating it when it does not
/// exist, and applies the pragmas: `journal_mode=WAL`, `synchronous=NORMAL`,
/// `busy_timeout=5000`, `foreign_keys=ON`, `journal_size_limit=64MiB`,
/// `temp_store=MEMORY`.
///
/// A new database file is created with mode 0600 before SQLite opens it, and SQLite
/// gives the `-wal` and `-shm` files the same mode, because conversations and command
/// output are private. The parent directory must exist.
///
/// Fails with [`StoreError::NotWal`] when SQLite keeps the file out of WAL mode, as it
/// does on file systems without shared memory: the single writer with concurrent
/// readers depends on it.
pub fn open(path: &Path) -> Result<Connection, StoreError> {
    create_private_if_missing(path)?;
    #[expect(
        clippy::disallowed_methods,
        reason = "efr_store::db::open is the replacement that clippy.toml names"
    )]
    let conn = Connection::open(path)
        .map_err(|source| StoreError::Open { path: path.to_path_buf(), source })?;
    let mode = set_journal_mode(&conn, "wal")?;
    if mode != "wal" {
        return Err(StoreError::NotWal { path: path.to_path_buf(), mode });
    }
    configure(&conn)?;
    Ok(conn)
}

/// Opens a new private in-memory database with the same pragmas as [`open`], except
/// that an in-memory database keeps its journal in memory instead of a write-ahead
/// log.
///
/// The database disappears when the connection closes, and no other connection can
/// reach it. `Store::open_in_memory` builds a whole store on one.
pub fn open_in_memory() -> Result<Connection, StoreError> {
    #[expect(
        clippy::disallowed_methods,
        reason = "efr_store::db::open_in_memory is the replacement that clippy.toml names"
    )]
    let conn = Connection::open_in_memory()
        .map_err(|source| StoreError::Open { path: PathBuf::from(":memory:"), source })?;
    configure(&conn)?;
    Ok(conn)
}

/// Opens the database at `path` read-only, for the reader pool. The database must
/// exist and be in WAL mode, which the writer's [`open`] guarantees.
///
/// `query_only` is set as well, so a write fails even through a statement that the
/// read-only flag would not catch, such as a pragma.
pub(crate) fn open_read_only(path: &Path) -> Result<Connection, StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .map_err(|source| StoreError::Open { path: path.to_path_buf(), source })?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|source| StoreError::Configure { pragma: "busy_timeout", source })?;
    set_pragma(&conn, "temp_store", "MEMORY")?;
    set_pragma(&conn, "query_only", "ON")?;
    Ok(conn)
}

/// The pragmas that every read-write connection gets, apart from the journal mode.
fn configure(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|source| StoreError::Configure { pragma: "busy_timeout", source })?;
    set_pragma(conn, "synchronous", "NORMAL")?;
    set_pragma(conn, "foreign_keys", "ON")?;
    set_pragma(conn, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;
    set_pragma(conn, "temp_store", "MEMORY")?;
    Ok(())
}

fn set_pragma(
    conn: &Connection,
    pragma: &'static str,
    value: impl rusqlite::ToSql,
) -> Result<(), StoreError> {
    conn.pragma_update(None, pragma, value)
        .map_err(|source| StoreError::Configure { pragma, source })
}

/// Sets the journal mode and returns the mode SQLite actually chose, in lowercase.
fn set_journal_mode(conn: &Connection, mode: &str) -> Result<String, StoreError> {
    conn.pragma_update_and_check(None, "journal_mode", mode, |row| row.get::<_, String>(0))
        .map(|chosen| chosen.to_ascii_lowercase())
        .map_err(|source| StoreError::Configure { pragma: "journal_mode", source })
}

/// Creates an empty file with mode 0600 at `path` unless something is already there.
/// SQLite treats an empty file as a new database.
fn create_private_if_missing(path: &Path) -> Result<(), StoreError> {
    match efr_stdx::fs::create_private(path) {
        Ok(_file) => Ok(()),
        Err(StdxError::CreateFile { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            Ok(())
        }
        Err(source) => Err(StoreError::CreateFile { path: path.to_path_buf(), source }),
    }
}

#[cfg(test)]
mod tests;
