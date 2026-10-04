use std::os::unix::fs::PermissionsExt as _;

use pretty_assertions::assert_eq;
use rusqlite::Connection;

use super::*;

fn pragma_i64(conn: &Connection, name: &str) -> i64 {
    conn.pragma_query_value(None, name, |row| row.get(0)).unwrap()
}

fn pragma_text(conn: &Connection, name: &str) -> String {
    conn.pragma_query_value(None, name, |row| row.get(0)).unwrap()
}

fn assert_shared_pragmas(conn: &Connection) {
    // synchronous: 1 is NORMAL. temp_store: 2 is MEMORY.
    assert_eq!(pragma_i64(conn, "synchronous"), 1);
    assert_eq!(pragma_i64(conn, "foreign_keys"), 1);
    assert_eq!(pragma_i64(conn, "busy_timeout"), 5000);
    assert_eq!(pragma_i64(conn, "journal_size_limit"), 64 * 1024 * 1024);
    assert_eq!(pragma_i64(conn, "temp_store"), 2);
}

#[test]
fn open_sets_every_pragma() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(&dir.path().join("efr.sqlite")).unwrap();
    assert_eq!(pragma_text(&conn, "journal_mode"), "wal");
    assert_shared_pragmas(&conn);
}

#[test]
fn open_in_memory_sets_every_pragma_but_wal() {
    let conn = open_in_memory().unwrap();
    assert_eq!(pragma_text(&conn, "journal_mode"), "memory");
    assert_shared_pragmas(&conn);
}

#[test]
fn open_creates_a_private_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let conn = open(&path).unwrap();
    conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let wal = dir.path().join("efr.sqlite-wal");
    let wal_mode = std::fs::metadata(&wal).unwrap().permissions().mode() & 0o777;
    assert_eq!(wal_mode, 0o600);
}

#[test]
fn open_keeps_an_existing_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (7)")
        .unwrap();
    let conn = open(&path).unwrap();
    let x: i64 = conn.query_row("SELECT x FROM t", [], |row| row.get(0)).unwrap();
    assert_eq!(x, 7);
}

#[test]
fn open_without_a_parent_directory_fails_with_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing").join("efr.sqlite");
    let error = open(&path).unwrap_err();
    assert!(matches!(error, StoreError::CreateFile { path: ref p, .. } if *p == path), "{error:?}");
}

#[test]
fn read_only_connections_refuse_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let writer = open(&path).unwrap();
    writer.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1)").unwrap();
    let reader = open_read_only(&path).unwrap();
    let x: i64 = reader.query_row("SELECT x FROM t", [], |row| row.get(0)).unwrap();
    assert_eq!(x, 1);
    assert!(reader.execute("INSERT INTO t VALUES (2)", []).is_err());
    assert_eq!(pragma_i64(&reader, "busy_timeout"), 5000);
}

#[test]
fn read_only_open_of_a_missing_database_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    assert!(matches!(open_read_only(&path), Err(StoreError::Open { .. })));
}
