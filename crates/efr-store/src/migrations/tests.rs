use std::os::unix::fs::PermissionsExt as _;

use pretty_assertions::assert_eq;
use rusqlite::Connection;

use super::*;
use crate::db;

fn tables(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .unwrap();
    stmt.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap()
}

#[test]
fn the_migrations_validate() {
    Migrations::new().validate().unwrap();
}

#[test]
fn every_file_in_the_directory_is_a_step_in_order() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/migrations");
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".sql"))
        .collect();
    names.sort();
    assert_eq!(names.len(), STEPS.len());
    for (index, name) in names.iter().enumerate() {
        let (number, noun) = name.trim_end_matches(".sql").split_once('_').unwrap();
        assert_eq!(number, format!("{:04}", index + 1), "{name} is out of sequence");
        assert!(
            noun.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{name} breaks the NNNN_<noun>.sql rule"
        );
    }
}

#[test]
fn a_new_database_migrates_to_the_latest_version_without_a_backup() {
    let mut conn = db::open_in_memory().unwrap();
    let report = Migrations::new().migrate(&mut conn, None).unwrap();
    assert_eq!(report, MigrationReport { from: 0, to: 4, backup: None });
    assert!(report.applied());
    assert_eq!(Migrations::version(&conn).unwrap(), 4);
    assert_eq!(
        tables(&conn),
        [
            "approvals",
            "conversations",
            "events",
            "outbox",
            "receipts",
            "recording_segments",
            "shells",
            "turns",
        ]
    );
}

#[test]
fn migrating_twice_does_nothing_the_second_time() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    let mut conn = db::open(&dir.path().join("efr.sqlite")).unwrap();
    Migrations::new().migrate(&mut conn, Some(&backups)).unwrap();
    let report = Migrations::new().migrate(&mut conn, Some(&backups)).unwrap();
    assert_eq!(report, MigrationReport { from: 4, to: 4, backup: None });
    assert!(!report.applied());
    assert!(!backups.exists(), "a new database needs no backup");
}

#[test]
fn an_existing_database_is_backed_up_before_it_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    let mut conn = db::open(&dir.path().join("efr.sqlite")).unwrap();
    Migrations::new().steps.to_version(&mut conn, 2).unwrap();
    conn.execute_batch(
        "INSERT INTO events (seq, kind, payload, created_at) \
         VALUES (1, 'login_completed', '{\"kind\":\"login_completed\",\"provider\":\"openai\"}', 0)",
    )
    .unwrap();

    let report = Migrations::new().migrate(&mut conn, Some(&backups)).unwrap();

    let backup = backups.join("efr.sqlite.2");
    assert_eq!(report, MigrationReport { from: 2, to: 4, backup: Some(backup.clone()) });
    assert_eq!(Migrations::version(&conn).unwrap(), 4);
    let mode = fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = fs::metadata(&backups).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    let copy = db::open(&backup).unwrap();
    assert_eq!(Migrations::version(&copy).unwrap(), 2);
    assert_eq!(count(&copy, "events"), 1);
    assert!(!tables(&copy).contains(&"receipts".to_owned()));
}

#[test]
fn a_leftover_temporary_backup_and_an_older_copy_are_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    fs::create_dir(&backups).unwrap();
    fs::write(backups.join(".efr.sqlite.1.tmp"), b"half a copy").unwrap();
    fs::write(backups.join("efr.sqlite.1"), b"an older copy").unwrap();
    let mut conn = db::open(&dir.path().join("efr.sqlite")).unwrap();
    Migrations::new().steps.to_version(&mut conn, 1).unwrap();

    Migrations::new().migrate(&mut conn, Some(&backups)).unwrap();

    assert!(!backups.join(".efr.sqlite.1.tmp").exists());
    let copy = db::open(&backups.join("efr.sqlite.1")).unwrap();
    assert_eq!(Migrations::version(&copy).unwrap(), 1);
}

#[test]
fn an_in_memory_database_gets_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    let mut conn = db::open_in_memory().unwrap();
    Migrations::new().steps.to_version(&mut conn, 1).unwrap();
    let report = Migrations::new().migrate(&mut conn, Some(&backups)).unwrap();
    assert_eq!(report.backup, None);
    assert!(!backups.exists());
}

#[test]
fn a_database_from_a_newer_build_is_refused() {
    let mut conn = db::open_in_memory().unwrap();
    conn.pragma_update(None, "user_version", 9).unwrap();
    let error = Migrations::new().migrate(&mut conn, None).unwrap_err();
    assert!(matches!(error, StoreError::SchemaTooNew { found: 9, supported: 4 }), "{error:?}");
}

static BROKEN: &[M<'static>] = &[
    M::up("CREATE TABLE first (x INTEGER);"),
    M::up("CREATE TABLE second (x INTEGER); THIS IS NOT SQL;"),
];

#[test]
fn a_failing_file_leaves_the_database_at_the_last_good_version() {
    let mut conn = db::open_in_memory().unwrap();
    let error = Migrations::from_steps(BROKEN).migrate(&mut conn, None).unwrap_err();
    assert!(matches!(error, StoreError::Migrate { .. }), "{error:?}");
    assert_eq!(Migrations::version(&conn).unwrap(), 1);
    assert_eq!(tables(&conn), ["first"]);
}

#[test]
fn events_cannot_be_changed_or_deleted() {
    let mut conn = db::open_in_memory().unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    conn.execute_batch(
        "INSERT INTO events (seq, kind, payload, created_at) VALUES (1, 'x', '{}', 0)",
    )
    .unwrap();
    assert!(conn.execute("UPDATE events SET kind = 'y'", []).is_err());
    assert!(conn.execute("DELETE FROM events", []).is_err());
    assert_eq!(count(&conn, "events"), 1);
}

#[test]
fn payload_columns_must_hold_json() {
    let mut conn = db::open_in_memory().unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    let result = conn.execute(
        "INSERT INTO events (seq, kind, payload, created_at) VALUES (1, 'x', 'not json', 0)",
        [],
    );
    assert!(result.is_err());
}
