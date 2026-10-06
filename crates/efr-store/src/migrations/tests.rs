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
    assert_eq!(report, MigrationReport { from: 0, to: 5, backup: None });
    assert!(report.applied());
    assert_eq!(Migrations::version(&conn).unwrap(), 5);
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
            "turn_messages",
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
    assert_eq!(report, MigrationReport { from: 5, to: 5, backup: None });
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
    assert_eq!(report, MigrationReport { from: 2, to: 5, backup: Some(backup.clone()) });
    assert_eq!(Migrations::version(&conn).unwrap(), 5);
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
    assert!(matches!(error, StoreError::SchemaTooNew { found: 9, supported: 5 }), "{error:?}");
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

#[test]
fn migrating_an_existing_database_rebuilds_the_projections() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open(&dir.path().join("efr.sqlite")).unwrap();
    Migrations::new().steps.to_version(&mut conn, 1).unwrap();
    let id = crate::testing::conversation(1);
    conn.execute(
        "INSERT INTO events (seq, conversation_id, kind, payload, created_at) \
         VALUES (1, ?1, 'conversation_created', '{\"kind\":\"conversation_created\",\"origin\":\"cli\"}', 0)",
        [id.to_string()],
    )
    .unwrap();

    Migrations::new().migrate(&mut conn, None).unwrap();

    let summary = crate::conversations::get(&conn, id).unwrap().unwrap();
    assert_eq!(summary.last_seq.get(), 1);
}

/// The checked-in databases, one per schema version: `fixtures/db/efr.sqlite.<n>`.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join("db")
}

fn fixture_conversation() -> efr_protocol::ConversationId {
    crate::testing::conversation(1)
}

#[test]
fn every_fixture_database_migrates_to_the_latest_version() {
    let latest = Migrations::new().latest();
    let mut names: Vec<String> = fs::read_dir(fixture_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    // A new migration ships with a fixture at its version.
    let expected: Vec<String> =
        (1..=latest).map(|version| format!("efr.sqlite.{version}")).collect();
    assert_eq!(names, expected);

    for version in 1..=latest {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("efr.sqlite");
        fs::copy(fixture_dir().join(format!("efr.sqlite.{version}")), &path).unwrap();
        let backups = dir.path().join("backups");
        let mut conn = db::open(&path).unwrap();

        let report = Migrations::new().migrate(&mut conn, Some(&backups)).unwrap();

        assert_eq!((report.from, report.to), (version, latest), "efr.sqlite.{version}");
        assert_eq!(report.backup.is_some(), version < latest, "efr.sqlite.{version}");
        let log = crate::events::read_after(&conn, efr_protocol::Seq::ZERO, 100).unwrap();
        assert!(log.len() >= 6, "efr.sqlite.{version} lost events");
        let summary = crate::conversations::get(&conn, fixture_conversation()).unwrap().unwrap();
        assert_eq!(summary.title.as_deref(), Some("update the system"), "efr.sqlite.{version}");
        assert_eq!(summary.status, efr_protocol::ConversationStatus::Idle);
        if version >= 3 {
            let receipt = crate::receipts::lookup(&conn, crate::testing::command(1)).unwrap();
            assert!(receipt.is_some(), "efr.sqlite.{version} lost its receipt");
            assert!(!crate::outbox::open_items(&conn).unwrap().is_empty());
        }
        let saved = crate::turn_messages::of_conversation(&conn, fixture_conversation()).unwrap();
        assert_eq!(saved.len(), usize::from(version >= 5), "efr.sqlite.{version}");
    }
}

/// The events every fixture holds: one finished turn and a login.
fn fixture_events() -> Vec<(Option<efr_protocol::ConversationId>, efr_protocol::Event)> {
    use efr_protocol::{Event, Usage};
    let id = Some(fixture_conversation());
    vec![
        (id, crate::testing::created(Some("pts-1"))),
        (id, crate::testing::queued(1, "update the system")),
        (id, crate::testing::started(1, "/etc/nixos")),
        (
            id,
            Event::AssistantMessageCompleted {
                turn_id: crate::testing::turn(1),
                index: 0,
                text: "Running nixos-rebuild switch.".to_owned(),
            },
        ),
        (
            id,
            Event::TurnCompleted {
                turn_id: crate::testing::turn(1),
                usage: Some(Usage { input_tokens: 812, output_tokens: 64 }),
            },
        ),
        (None, Event::LoginCompleted { provider: "openai".to_owned() }),
    ]
}

/// Leaves the database as one small file: no write-ahead log, 1 KiB pages.
fn compact(conn: &Connection) {
    conn.pragma_update_and_check(None, "journal_mode", "DELETE", |_| Ok(())).unwrap();
    conn.pragma_update(None, "page_size", 1024).unwrap();
    conn.execute_batch("VACUUM").unwrap();
}

/// A database at an old `version`, written the way a build of that version would
/// have: events, the projections that existed, and receipts and outbox rows from
/// version 3.
fn write_old_fixture(path: &Path, version: u32) {
    let mut conn = db::open(path).unwrap();
    Migrations::new().steps.to_version(&mut conn, version as usize).unwrap();
    let start = crate::testing::start();
    for (index, (conversation_id, event)) in fixture_events().into_iter().enumerate() {
        let at = start.checked_add(jiff::SignedDuration::from_secs(index as i64)).unwrap();
        let envelope = efr_protocol::EventEnvelope {
            seq: efr_protocol::Seq::new(index as u64 + 1),
            conversation_id,
            at: crate::sql::truncate_to_micros(at),
            event,
        };
        crate::events::insert(&conn, &envelope).unwrap();
    }
    let id = fixture_conversation().to_string();
    let turn = crate::testing::turn(1).to_string();
    if version >= 2 {
        conn.execute(
            "INSERT INTO conversations (id, origin, title, status, tty, cwd, scope, created_at, \
             updated_at, last_seq) VALUES (?1, 'shell', 'update the system', 'idle', 'pts-1', \
             '/etc/nixos', '{\"kind\":\"path\",\"value\":\"/etc/nixos\"}', 0, 4, 5)",
            [&id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turns (id, conversation_id, command_id, prompt, status, queued_seq, \
             started_at, ended_at, last_seq) VALUES (?1, ?2, ?3, 'update the system', \
             'completed', 2, 2, 4, 5)",
            [&turn, &id, &crate::testing::command(1).to_string()],
        )
        .unwrap();
    }
    if version >= 3 {
        conn.execute(
            "INSERT INTO receipts (command_id, method, outcome, result, seq, created_at) \
             VALUES (?1, 'prompt.send', 'accepted', '{\"queued\":false}', 2, 1)",
            [&crate::testing::command(1).to_string()],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO outbox (kind, payload, replay_safe, created_at, claimed_at) \
             VALUES ('notify.tty', '{\"tty\":\"pts-1\"}', 1, 1, 2);
             INSERT INTO outbox (kind, payload, replay_safe, created_at) \
             VALUES ('provider.turn', '{}', 0, 1);",
        )
        .unwrap();
    }
    compact(&conn);
}

/// A database at the latest version, written through the store itself.
async fn write_latest_fixture(path: &Path, recordings: &Path) {
    use efr_protocol::{ApprovalDecision, Event, Origin};
    use serde_json::json;

    use crate::outbox::NewOutboxItem;
    use crate::receipts::NewReceipt;
    use crate::{Batch, Store, StoreConfig};

    let clock = crate::testing::TestClock::new();
    let mut config = StoreConfig::in_data_dir(path.parent().unwrap());
    config.path = path.to_path_buf();
    config.backups = None;
    let store = Store::open(config, clock.clone()).await.unwrap();
    let id = fixture_conversation();
    let mut events = fixture_events().into_iter();
    let mut first = Batch::new();
    for (conversation_id, event) in events.by_ref().take(2) {
        first = first.event(conversation_id.unwrap(), event);
    }
    let first = first
        .event(
            id,
            Event::ShellStarted {
                pty_id: crate::testing::pty(1),
                cwd: "/etc/nixos".into(),
                pid: Some(4242),
            },
        )
        .receipt(NewReceipt::accepted(
            crate::testing::command(1),
            "prompt.send",
            json!({ "queued": false }),
        ))
        .enqueue(NewOutboxItem::replay_safe("notify.tty", json!({ "tty": "pts-1" })))
        .enqueue(NewOutboxItem::process_bound("provider.turn", json!({})));
    store.writer().append(first).await.unwrap();
    let approval = [
        Event::ApprovalRequested {
            turn_id: crate::testing::turn(1),
            call_id: crate::testing::call(1),
            summary: "nixos-rebuild switch".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
        },
        Event::ApprovalResolved {
            turn_id: crate::testing::turn(1),
            call_id: crate::testing::call(1),
            decision: ApprovalDecision::Allow,
            origin: Origin::Shell,
        },
    ];
    let mut rest = Batch::new();
    for (index, (conversation_id, event)) in events.enumerate() {
        rest = match conversation_id {
            Some(id) => rest.event(id, event),
            None => rest.global_event(event),
        };
        if index == 0 {
            for event in approval.clone() {
                rest = rest.event(id, event);
            }
        }
    }
    let messages = crate::turn_messages::NewTurnMessages::new(
        id,
        crate::testing::turn(1),
        "openai-subscription",
        "gpt-5.5",
        vec![
            json!({ "role": "user", "content": [{ "kind": "text", "text": "update the system" }] }),
            json!({
                "role": "assistant",
                "content": [{ "kind": "text", "text": "Running nixos-rebuild switch." }],
                "provider_raw": [{ "type": "reasoning", "encrypted_content": "opaque" }],
            }),
        ],
        50,
    );
    store.writer().append(rest.turn_messages(messages)).await.unwrap();
    let recordings = crate::recording::Recordings::new(
        recordings,
        store.writer().clone(),
        store.readers().clone(),
        clock,
    );
    let mut recorder = recordings.start(crate::testing::pty(1)).await.unwrap();
    recorder.append(b"$ nixos-rebuild switch\r\n").await.unwrap();
    recorder.close().await.unwrap();
    drop(recordings);
    store.close().await;
    compact(&db::open(path).unwrap());
}

/// Writes `fixtures/db/`. Run it only to add the fixture of a new version:
/// `cargo nextest run -p efr-store --run-ignored only write_fixture_databases`.
/// A fixture of a shipped version is never rewritten.
#[tokio::test]
#[ignore = "writes fixtures/db; run by hand when a migration is added"]
async fn write_fixture_databases() {
    let latest = Migrations::new().latest();
    fs::create_dir_all(fixture_dir()).unwrap();
    for version in 1..=latest {
        let target = fixture_dir().join(format!("efr.sqlite.{version}"));
        if target.exists() {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("efr.sqlite");
        if version == latest {
            write_latest_fixture(&path, &dir.path().join("recordings")).await;
        } else {
            write_old_fixture(&path, version);
        }
        fs::copy(&path, &target).unwrap();
    }
}
