use std::path::Path;

use efr_protocol::{Event, Seq};
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

use super::*;
use crate::testing::{self, TestClock};
use crate::writer::DEFAULT_BROADCAST_CAPACITY;
use crate::{Batch, Migrations, StoreWriter, events};

fn login(provider: &str) -> Event {
    Event::LoginCompleted { provider: provider.to_owned() }
}

fn file_writer(path: &Path) -> (WriterHandle, StoreWriter) {
    let mut conn = db::open(path).unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    StoreWriter::spawn(conn, TestClock::new(), DEFAULT_BROADCAST_CAPACITY).unwrap()
}

fn idle_connections(readers: &Readers) -> usize {
    let Backend::Pool(pool) = &readers.backend else { panic!("not a pool") };
    pool.idle.lock().unwrap().len()
}

#[tokio::test]
async fn pool_readers_see_committed_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let (writer, _thread) = file_writer(&path);
    let readers = Readers::open(&path, 2);

    let committed = writer.append(Batch::new().global_event(login("a"))).await.unwrap();
    let read = readers.with(|conn| events::read_after(conn, Seq::ZERO, 10)).await.unwrap();

    assert_eq!(read, committed.events());
}

#[tokio::test]
async fn pool_connections_are_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let (_writer, _thread) = file_writer(&path);
    let readers = Readers::open(&path, 4);

    readers.with(events::last_seq).await.unwrap();
    readers.with(events::last_seq).await.unwrap();

    assert_eq!(idle_connections(&readers), 1);
}

#[tokio::test]
async fn pool_readers_refuse_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let (_writer, _thread) = file_writer(&path);
    let readers = Readers::open(&path, 1);

    let result = readers
        .with(|conn| {
            conn.execute("INSERT INTO outbox (kind, payload, replay_safe, created_at) VALUES ('x', '{}', 1, 0)", [])?;
            Ok(())
        })
        .await;

    assert!(matches!(result, Err(StoreError::Sqlite { .. })), "{result:?}");
    assert_eq!(idle_connections(&readers), 1, "the connection survives a failed closure");
}

#[tokio::test]
async fn a_read_sees_one_snapshot_while_the_writer_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let (writer, _thread) = file_writer(&path);
    writer.append(Batch::new().global_event(login("a"))).await.unwrap();
    let readers = Readers::open(&path, 1);
    let (first_read_tx, first_read_rx) = oneshot::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();

    let read = tokio::spawn(async move {
        readers
            .with(move |conn| {
                let before = events::last_seq(conn)?;
                first_read_tx.send(()).unwrap();
                go_rx.recv().unwrap();
                let after = events::last_seq(conn)?;
                Ok((before, after))
            })
            .await
    });
    first_read_rx.await.unwrap();
    writer.append(Batch::new().global_event(login("b"))).await.unwrap();
    go_tx.send(()).unwrap();

    let (before, after) = read.await.unwrap().unwrap();
    assert_eq!((before, after), (Seq::new(1), Seq::new(1)));
}

#[tokio::test]
async fn writer_readers_refuse_writes_and_leave_the_writer_writable() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let readers = Readers::through_writer(writer.clone());

    let result = readers
        .with(|conn| {
            conn.execute("INSERT INTO outbox (kind, payload, replay_safe, created_at) VALUES ('x', '{}', 1, 0)", [])?;
            Ok(())
        })
        .await;
    let committed = writer.append(Batch::new().global_event(login("a"))).await.unwrap();
    let read = readers.with(|conn| events::read_after(conn, Seq::ZERO, 10)).await.unwrap();

    assert!(matches!(result, Err(StoreError::Sqlite { .. })), "{result:?}");
    assert_eq!(read, committed.events());
}

#[tokio::test]
async fn a_writer_reader_that_panics_leaves_the_writer_writable() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let readers = Readers::through_writer(writer.clone());

    let result = readers.with(|_conn| -> Result<(), StoreError> { panic!("reader bug") }).await;
    let committed = writer.append(Batch::new().global_event(login("a"))).await;

    assert!(matches!(result, Err(StoreError::TaskPanicked { .. })), "{result:?}");
    assert!(committed.is_ok(), "{committed:?}");
}
