//! `turn.interrupt` over a `TestDaemon`: two phases, the request at once and the end
//! when the model's stream has stopped, with the text that already streamed kept.

use efr_protocol::{
    ConversationStatus, ConversationsList, ConversationsListResult, ErrorCode, Event, Method,
    TurnInterrupt, TurnInterruptResult,
};
use efr_test_daemon::{ClientError, Replay, command_id};
use pretty_assertions::assert_eq;
use serde_json::Value;

#[tokio::test]
async fn interrupt_mid_stream() {
    let replay = Replay::run("interrupt_mid_stream").await.unwrap();

    let result: TurnInterruptResult =
        serde_json::from_value(replay.result(2).unwrap().clone().unwrap()).unwrap();
    let turn = replay.bindings().get("<turn:1>").unwrap();
    assert_eq!(result.turn_id.to_string(), turn);
    let events = replay.events().await.unwrap();
    let requested = events.iter().position(|e| e.event.kind() == "turn_interrupt_requested");
    let interrupted = events.iter().position(|e| e.event.kind() == "turn_interrupted");
    assert!(requested.unwrap() < interrupted.unwrap(), "the request, then the stop");
    assert_eq!(events.last().unwrap().event.kind(), "turn_interrupted");
    let kept = events.iter().find_map(|envelope| match &envelope.event {
        Event::AssistantMessageCompleted { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(kept.as_deref(), Some("Counting: one"), "only what streamed before the stop");
    let list: ConversationsListResult = replay
        .client()
        .call(Method::ConversationsList(ConversationsList::default()))
        .await
        .unwrap();
    assert_eq!(list.conversations[0].status, ConversationStatus::Idle);
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn interrupting_a_conversation_with_no_running_turn_is_a_conflict() {
    let replay = Replay::run("interrupt_mid_stream").await.unwrap();

    let again = Method::TurnInterrupt(TurnInterrupt {
        command_id: command_id(3),
        conversation_id: replay.conversation().unwrap(),
        turn_id: None,
        resend_steers: Vec::new(),
        withdraw: Vec::new(),
    });
    let refused = replay.client().call::<Value>(again).await;

    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::Conflict);
    replay.stop().await.unwrap();
}
