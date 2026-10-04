use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock as _;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle};

fn ids(items: &[OutboxItem]) -> Vec<i64> {
    items.iter().map(|item| item.id.get()).collect()
}

async fn enqueue_all(writer: &WriterHandle, items: Vec<NewOutboxItem>) {
    let mut batch = Batch::new();
    for item in items {
        batch = batch.enqueue(item);
    }
    writer.append(batch).await.unwrap();
}

#[tokio::test]
async fn items_are_claimed_oldest_first_up_to_the_limit_and_only_once() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    enqueue_all(
        &writer,
        (1..=3).map(|n| NewOutboxItem::replay_safe("notify.tty", json!({ "n": n }))).collect(),
    )
    .await;
    clock.advance(Duration::from_secs(1));

    let first = writer.outbox_claim(2).await.unwrap();
    let second = writer.outbox_claim(2).await.unwrap();
    let third = writer.outbox_claim(2).await.unwrap();

    assert_eq!(ids(&first), [1, 2]);
    assert_eq!(first[0].kind, "notify.tty");
    assert_eq!(first[0].payload, json!({ "n": 1 }));
    assert!(first[0].replay_safe);
    let now = sql::truncate_to_micros(clock.now());
    assert_eq!(first[0].claimed_at, Some(now));
    assert!(first[0].created_at < now);
    assert_eq!(ids(&second), [3]);
    assert_eq!(third, []);
}

#[tokio::test]
async fn a_done_item_leaves_the_open_list() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    enqueue_all(
        &writer,
        vec![NewOutboxItem::replay_safe("a", json!(1)), NewOutboxItem::replay_safe("b", json!(2))],
    )
    .await;
    let claimed = writer.outbox_claim(1).await.unwrap();

    writer.outbox_done(claimed[0].id).await.unwrap();

    let open = on_writer(&writer, open_items).await.unwrap();
    assert_eq!(ids(&open), [2]);
    assert_eq!(open[0].claimed_at, None);
}

#[tokio::test]
async fn only_a_claimed_unfinished_item_can_be_done() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    enqueue_all(
        &writer,
        vec![NewOutboxItem::replay_safe("a", json!(1)), NewOutboxItem::replay_safe("b", json!(2))],
    )
    .await;
    let claimed = writer.outbox_claim(1).await.unwrap()[0].id;
    writer.outbox_done(claimed).await.unwrap();

    let twice = writer.outbox_done(claimed).await.unwrap_err();
    let unclaimed = writer.outbox_done(OutboxId(2)).await.unwrap_err();
    let unknown = writer.outbox_done(OutboxId(99)).await.unwrap_err();

    for error in [twice, unclaimed, unknown] {
        assert!(matches!(error, StoreError::OutboxItemNotClaimed { .. }), "{error:?}");
    }
}

#[tokio::test]
async fn a_restart_cancels_process_bound_items_and_requeues_replay_safe_ones() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    enqueue_all(
        &writer,
        vec![
            NewOutboxItem::process_bound("provider.turn", json!({ "claimed": true })),
            NewOutboxItem::replay_safe("notify.tty", json!({ "claimed": true })),
            NewOutboxItem::replay_safe("notify.tty", json!({ "claimed": true, "done": true })),
            NewOutboxItem::process_bound("provider.turn", json!({ "claimed": false })),
            NewOutboxItem::replay_safe("notify.tty", json!({ "claimed": false })),
        ],
    )
    .await;
    let claimed = writer.outbox_claim(3).await.unwrap();
    writer.outbox_done(claimed[2].id).await.unwrap();

    let reconciled = writer.outbox_cancel_process_bound().await.unwrap();

    assert_eq!(reconciled, OutboxReconciled { cancelled: 2, requeued: 1 });
    let open = on_writer(&writer, open_items).await.unwrap();
    assert_eq!(ids(&open), [2, 5]);
    assert!(open.iter().all(|item| item.claimed_at.is_none()));
    assert_eq!(ids(&writer.outbox_claim(10).await.unwrap()), [2, 5]);
}

#[tokio::test]
async fn an_item_is_enqueued_only_when_its_batch_commits() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let failed = writer
        .append(
            Batch::new()
                .event(testing::conversation(1), testing::queued(1, "no such conversation"))
                .enqueue(NewOutboxItem::process_bound("provider.turn", json!({}))),
        )
        .await;

    assert!(failed.is_err());
    assert_eq!(on_writer(&writer, open_items).await.unwrap(), []);
}
