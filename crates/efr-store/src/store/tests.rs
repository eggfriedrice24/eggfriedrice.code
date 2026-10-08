use std::os::unix::fs::PermissionsExt as _;

use efr_protocol::{Event, Seq};
use pretty_assertions::assert_eq;

use super::*;
use crate::testing::TestClock;
use crate::{Batch, events};

fn login(provider: &str) -> Event {
    Event::LoginCompleted { provider: provider.to_owned() }
}

#[test]
fn the_data_dir_layout_is_the_database_and_its_backups() {
    let config = StoreConfig::in_data_dir(Path::new("/d/efr"));
    assert_eq!(config.path, Path::new("/d/efr/efr.sqlite"));
    assert_eq!(config.backups.as_deref(), Some(Path::new("/d/efr/backups")));
    assert_eq!(config.readers, DEFAULT_READERS);
    assert_eq!(config.broadcast_capacity, DEFAULT_BROADCAST_CAPACITY);
}

#[tokio::test]
async fn open_creates_a_private_data_dir_and_a_migrated_database() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data").join("efr");

    let store = Store::open(StoreConfig::in_data_dir(&data), TestClock::new()).await.unwrap();

    assert_eq!(store.migration(), &MigrationReport { from: 0, to: 6, backup: None });
    let mode = std::fs::metadata(&data).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let committed = store.writer().append(Batch::new().global_event(login("a"))).await.unwrap();
    let read = store.readers().with(|conn| events::read_after(conn, Seq::ZERO, 10)).await.unwrap();
    assert_eq!(read, committed.events());
}

#[tokio::test]
async fn a_reopened_store_continues_its_log_without_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let config = StoreConfig::in_data_dir(dir.path());
    let store = Store::open(config.clone(), TestClock::new()).await.unwrap();
    store.writer().append(Batch::new().global_event(login("a"))).await.unwrap();
    store.close().await;

    let store = Store::open(config, TestClock::new()).await.unwrap();
    let committed = store.writer().append(Batch::new().global_event(login("b"))).await.unwrap();

    assert_eq!(store.migration(), &MigrationReport { from: 6, to: 6, backup: None });
    assert_eq!(committed.first_seq(), Some(Seq::new(2)));
}

#[tokio::test]
async fn an_older_database_is_backed_up_in_the_data_dir_before_it_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let config = StoreConfig::in_data_dir(dir.path());
    {
        let mut conn = db::open(&config.path).unwrap();
        let steps = rusqlite_migration::Migrations::from_slice(crate::migrations::STEPS);
        steps.to_version(&mut conn, 3).unwrap();
    }

    let store = Store::open(config, TestClock::new()).await.unwrap();

    let backup = dir.path().join("backups").join("efr.sqlite.3");
    assert_eq!(
        store.migration(),
        &MigrationReport { from: 3, to: 6, backup: Some(backup.clone()) }
    );
    assert!(backup.is_file());
}

#[tokio::test]
async fn close_waits_until_the_database_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let config = StoreConfig::in_data_dir(dir.path());
    let store = Store::open(config, TestClock::new()).await.unwrap();
    store.writer().append(Batch::new().global_event(login("a"))).await.unwrap();
    store.readers().with(events::last_seq).await.unwrap();

    store.close().await;

    // The last connection to close checkpoints the write-ahead log and removes it.
    assert!(!dir.path().join("efr.sqlite-wal").exists());
}

#[tokio::test]
async fn an_in_memory_store_reads_through_its_writer() {
    let store = Store::open_in_memory(TestClock::new()).await.unwrap();

    let committed = store.writer().append(Batch::new().global_event(login("a"))).await.unwrap();
    let read = store.readers().with(|conn| events::read_after(conn, Seq::ZERO, 10)).await.unwrap();

    assert_eq!(store.migration(), &MigrationReport { from: 0, to: 6, backup: None });
    assert_eq!(read, committed.events());
    store.close().await;
}
