use std::time::Duration;

use efr_protocol::{ConversationId, Event, Origin, PtyId, Seq};
use efr_store::Batch;
use pretty_assertions::assert_eq;

use super::TestStore;
use crate::TestClock;

fn conversation() -> ConversationId {
    conversation_n(1)
}

fn conversation_n(n: u64) -> ConversationId {
    format!("00000001-0000-7000-8000-{n:012x}").parse().unwrap()
}

fn created() -> Event {
    Event::ConversationCreated { origin: Origin::Shell, tty: None }
}

#[tokio::test]
async fn a_new_store_is_migrated_and_empty() {
    let store = TestStore::open(TestClock::new().shared()).await.unwrap();
    assert!(store.store().migration().applied());
    assert_eq!(store.events().await.unwrap(), []);
    store.close().await;
}

#[tokio::test]
async fn events_carry_the_time_of_the_test_clock() {
    let clock = TestClock::new();
    let store = TestStore::open(clock.shared()).await.unwrap();
    store.writer().append(Batch::new().event(conversation(), created())).await.unwrap();
    clock.advance(Duration::from_secs(90));
    store.writer().append(Batch::new().event(conversation_n(2), created())).await.unwrap();

    let events = store.events().await.unwrap();
    let seen: Vec<(u64, String)> =
        events.iter().map(|envelope| (envelope.seq.get(), envelope.at.to_string())).collect();
    assert_eq!(
        seen,
        [(1, "2026-10-04T12:00:00Z".to_owned()), (2, "2026-10-04T12:01:30Z".to_owned())]
    );
    assert_eq!(events[0].conversation_id, Some(conversation()));
    assert_eq!(events[0].event, created());
    store.close().await;
}

#[tokio::test]
async fn two_stores_do_not_share_a_database() {
    let clock = TestClock::new();
    let one = TestStore::open(clock.shared()).await.unwrap();
    let two = TestStore::open(clock.shared()).await.unwrap();
    one.writer().append(Batch::new().event(conversation(), created())).await.unwrap();
    assert_eq!(two.events().await.unwrap(), []);
    one.close().await;
    two.close().await;
}

#[tokio::test]
async fn recordings_go_to_a_temporary_directory_and_read_back() {
    let store = TestStore::open(TestClock::new().shared()).await.unwrap();
    let pty: PtyId = "00000005-0000-7000-8000-000000000001".parse().unwrap();
    let root = store.recordings().root().to_path_buf();
    assert!(root.is_dir());

    let mut writer = store.recordings().start(pty).await.unwrap();
    writer.append(b"$ ls\r\n").await.unwrap();
    writer.close().await.unwrap();
    let range = store.recordings().read_range(pty, Seq::ZERO, Seq::new(u64::MAX)).await.unwrap();
    assert_eq!(range.bytes(), b"$ ls\r\n");

    store.close().await;
    assert!(!root.exists());
}
