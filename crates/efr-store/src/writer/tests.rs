use std::time::Duration;

use efr_protocol::Event;
use pretty_assertions::assert_eq;
use tokio::sync::broadcast::error::TryRecvError;

use super::*;
use crate::testing::{self, TestClock};
use crate::{Migrations, db};

fn login(provider: &str) -> Event {
    Event::LoginCompleted { provider: provider.to_owned() }
}

fn seqs(committed: &Committed) -> Vec<u64> {
    committed.events().iter().map(|envelope| envelope.seq.get()).collect()
}

#[tokio::test]
async fn sequence_numbers_start_at_one_and_follow_on_across_batches() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let first = writer.append(Batch::new().global_event(login("a")).global_event(login("b"))).await;
    let second = writer.append(Batch::new().global_event(login("c"))).await;

    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(seqs(&first), [1, 2]);
    assert_eq!(first.first_seq(), Some(Seq::new(1)));
    assert_eq!(first.last_seq(), Seq::new(2));
    assert_eq!(seqs(&second), [3]);
    assert_eq!(second.last_seq(), Seq::new(3));
}

#[tokio::test]
async fn subscribers_receive_every_batch_in_commit_order() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let mut receiver = writer.subscribe();

    let first = writer.append(Batch::new().global_event(login("a"))).await.unwrap();
    let second = writer.append(Batch::new().global_event(login("b"))).await.unwrap();

    assert_eq!(receiver.recv().await.unwrap(), first);
    assert_eq!(receiver.recv().await.unwrap(), second);
}

#[tokio::test]
async fn an_empty_batch_commits_nothing_and_broadcasts_nothing() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.append(Batch::new().global_event(login("a"))).await.unwrap();
    let mut receiver = writer.subscribe();

    let committed = writer.append(Batch::new()).await.unwrap();

    assert_eq!(committed.events(), []);
    assert_eq!(committed.first_seq(), None);
    assert_eq!(committed.last_seq(), Seq::new(1));
    assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn events_of_a_batch_share_the_clock_time_to_the_microsecond() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    clock.advance(Duration::from_secs(90));

    let committed = writer
        .append(Batch::new().global_event(login("a")).global_event(login("b")))
        .await
        .unwrap();

    let expected = sql::truncate_to_micros(clock.now());
    assert_ne!(expected, clock.now(), "the test clock carries nanoseconds");
    for envelope in committed.events() {
        assert_eq!(envelope.at, expected);
    }
}

#[tokio::test]
async fn the_writer_continues_after_the_newest_event_when_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr.sqlite");
    let spawn = || {
        let mut conn = db::open(&path).unwrap();
        Migrations::new().migrate(&mut conn, None).unwrap();
        StoreWriter::spawn(conn, TestClock::new(), DEFAULT_BROADCAST_CAPACITY).unwrap()
    };

    let (writer, thread) = spawn();
    writer.append(Batch::new().global_event(login("a")).global_event(login("b"))).await.unwrap();
    drop(writer);
    thread.join().await;

    let (writer, _thread) = spawn();
    let committed = writer.append(Batch::new().global_event(login("c"))).await.unwrap();
    assert_eq!(seqs(&committed), [3]);
}

#[tokio::test]
async fn a_panicking_job_does_not_stop_the_writer() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let error =
        writer.run(|_state| -> Result<(), StoreError> { panic!("boom") }).await.unwrap_err();
    let committed = writer.append(Batch::new().global_event(login("a"))).await.unwrap();

    assert!(matches!(error, StoreError::TaskPanicked { task: "writer" }), "{error:?}");
    assert_eq!(seqs(&committed), [1]);
}

#[tokio::test]
async fn a_job_that_panics_inside_a_transaction_rolls_it_back() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let _ = writer
        .run(|state| -> Result<(), StoreError> {
            let tx = state.conn.transaction()?;
            tx.execute(
                "INSERT INTO events (seq, kind, payload, created_at) VALUES (1, 'x', '{}', 0)",
                [],
            )?;
            panic!("before the commit")
        })
        .await;
    let committed = writer.append(Batch::new().global_event(login("a"))).await.unwrap();

    assert_eq!(seqs(&committed), [1]);
}

#[tokio::test]
async fn join_returns_once_every_handle_is_gone() {
    let (writer, thread) = testing::memory_writer(TestClock::new());
    let clone = writer.clone();
    drop(writer);
    clone.append(Batch::new().global_event(login("a"))).await.unwrap();
    drop(clone);

    thread.join().await;
}

#[tokio::test]
async fn a_failed_batch_writes_nothing_and_uses_up_no_sequence_numbers() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();
    let mut receiver = writer.subscribe();

    let failed = writer
        .append(
            Batch::new()
                .event(id, testing::queued(1, "fine on its own"))
                .event(testing::conversation(2), testing::queued(2, "unknown conversation")),
        )
        .await;
    let committed =
        writer.append(Batch::new().event(id, testing::queued(3, "next"))).await.unwrap();

    assert!(failed.is_err());
    assert_eq!(seqs(&committed), [2]);
    assert_eq!(receiver.recv().await.unwrap(), committed, "the failed batch is never broadcast");
}
