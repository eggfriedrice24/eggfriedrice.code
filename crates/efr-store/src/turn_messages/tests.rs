use efr_protocol::{ConversationId, Event};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, StoreError, WriterHandle};

fn messages(turn: u64) -> Vec<Value> {
    vec![
        json!({ "role": "user", "content": [{ "kind": "text", "text": format!("prompt {turn}") }] }),
        json!({
            "role": "assistant",
            "content": [{ "kind": "text", "text": "done" }],
            "provider_raw": [{ "type": "reasoning", "encrypted_content": format!("opaque {turn}") }],
        }),
    ]
}

fn saved(id: ConversationId, turn: u64, keep: usize) -> NewTurnMessages {
    NewTurnMessages::new(id, testing::turn(turn), "replay", "test-model", messages(turn), keep)
}

/// Runs turn `turn` of `id` to its end, saving its messages in the same batch.
async fn finish_turn(writer: &WriterHandle, id: ConversationId, turn: u64, keep: usize) {
    writer
        .append(
            Batch::new()
                .event(id, testing::queued(turn, "go"))
                .event(id, testing::started(turn, "/")),
        )
        .await
        .unwrap();
    writer
        .append(
            Batch::new()
                .event(
                    id,
                    Event::TurnCompleted {
                        turn_id: testing::turn(turn),
                        usage: None,
                        changes: None,
                        context: None,
                    },
                )
                .turn_messages(saved(id, turn, keep)),
        )
        .await
        .unwrap();
}

async fn read(writer: &WriterHandle, id: ConversationId) -> Vec<TurnMessages> {
    on_writer(writer, move |conn| of_conversation(conn, id)).await.unwrap()
}

#[tokio::test]
async fn a_turns_messages_come_back_exactly_with_its_provider_and_model() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();

    finish_turn(&writer, id, 1, 50).await;

    assert_eq!(
        read(&writer, id).await,
        [TurnMessages {
            turn_id: testing::turn(1),
            provider: "replay".to_owned(),
            model: "test-model".to_owned(),
            messages: messages(1),
        }]
    );
    assert_eq!(read(&writer, testing::conversation(2)).await, []);
}

#[tokio::test]
async fn only_the_newest_turns_of_a_conversation_are_kept() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let (one, two) = (testing::conversation(1), testing::conversation(2));
    writer
        .append(Batch::new().event(one, testing::created(None)).event(two, testing::created(None)))
        .await
        .unwrap();
    finish_turn(&writer, two, 9, 2).await;

    for turn in 1..=4 {
        finish_turn(&writer, one, turn, 2).await;
    }

    let kept: Vec<TurnId> = read(&writer, one).await.iter().map(|turn| turn.turn_id).collect();
    assert_eq!(kept, [testing::turn(3), testing::turn(4)], "the oldest go first");
    assert_eq!(read(&writer, two).await.len(), 1, "another conversation keeps its own");
}

#[tokio::test]
async fn a_history_of_no_turns_keeps_nothing() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();
    finish_turn(&writer, id, 1, 50).await;

    finish_turn(&writer, id, 2, 0).await;

    assert_eq!(read(&writer, id).await, []);
}

#[tokio::test]
async fn messages_are_saved_only_when_their_batch_commits() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);

    // The conversation was never created, so the batch fails as a whole.
    let failed = writer
        .append(
            Batch::new()
                .event(
                    id,
                    Event::TurnCompleted {
                        turn_id: testing::turn(1),
                        usage: None,
                        changes: None,
                        context: None,
                    },
                )
                .turn_messages(saved(id, 1, 50)),
        )
        .await;

    assert!(matches!(failed, Err(StoreError::UnknownConversation { .. })), "{failed:?}");
    assert_eq!(read(&writer, id).await, []);
}

#[tokio::test]
async fn a_projection_rebuild_leaves_the_messages_alone() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();
    finish_turn(&writer, id, 1, 50).await;

    writer.rebuild_projections().await.unwrap();

    assert_eq!(read(&writer, id).await.len(), 1);
}
