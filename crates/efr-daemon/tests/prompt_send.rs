//! `prompt.send` over a `TestDaemon`: a whole turn, a prompt queued behind a running
//! turn, the scope following the user between turns, and the refusals.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

use efr_protocol::{
    ConversationStatus, ConversationsList, ConversationsListResult, ErrorCode, Event, Method,
    PromptSend, PromptSendResult, Scope,
};
use efr_test_daemon::{ClientError, Replay, TTY, TestDaemon};
use pretty_assertions::assert_eq;
use serde_json::Value;

fn sent(replay: &Replay, frame: u64) -> PromptSendResult {
    let result = replay.result(frame).unwrap().clone().unwrap();
    serde_json::from_value(result).unwrap()
}

fn kinds(events: &[efr_protocol::EventEnvelope]) -> Vec<&str> {
    events.iter().map(|envelope| envelope.event.kind()).collect()
}

#[tokio::test]
async fn single_turn_text() {
    let replay = Replay::run("single_turn_text").await.unwrap();

    let result = sent(&replay, 1);
    assert!(!result.queued);
    assert_eq!(Some(result.conversation_id), replay.conversation());
    let list: ConversationsListResult = replay
        .client()
        .call(Method::ConversationsList(ConversationsList::default()))
        .await
        .unwrap();
    let [summary] = list.conversations.as_slice() else { panic!("{list:?}") };
    assert_eq!(summary.id, result.conversation_id);
    assert_eq!(summary.title.as_deref(), Some("say hello"));
    assert_eq!(summary.tty.as_deref(), Some(TTY));
    assert_eq!(summary.status, ConversationStatus::Idle);
    let events = replay.events().await.unwrap();
    assert_eq!(
        kinds(&events),
        [
            "conversation_created",
            "prompt_queued",
            "turn_started",
            "assistant_message_updated",
            "assistant_message_completed",
            "turn_completed",
        ]
    );
    assert_eq!(events[1].seq, result.seq, "the result names the prompt's own event");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn queue_second_prompt() {
    let replay = Replay::run("queue_second_prompt").await.unwrap();

    let (first, second) = (sent(&replay, 1), sent(&replay, 2));
    assert!(!first.queued);
    assert!(second.queued, "the second prompt arrived while the first turn ran");
    assert_eq!(second.conversation_id, first.conversation_id, "the terminal's conversation");
    assert_ne!(second.turn_id, first.turn_id);
    let events = replay.events().await.unwrap();
    let started: Vec<_> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .collect();
    assert_eq!(started, [first.turn_id, second.turn_id], "turns run one after the other");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn cwd_move_between_turns() {
    let replay = Replay::run("cwd_move_between_turns").await.unwrap();

    let cwd = replay.daemon().cwd().to_path_buf();
    let events = replay.events().await.unwrap();
    let started: Vec<_> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::TurnStarted { cwd, scope, .. } => Some((cwd.clone(), scope.clone())),
            _ => None,
        })
        .collect();
    let project = replay.scenario().spec().project.unwrap().0.parse().unwrap();
    assert_eq!(
        started,
        [(cwd.join("alpha"), Scope::Machine), (cwd.join("beta"), Scope::Project(project))]
    );
    let changed = events.iter().filter(|envelope| envelope.event.kind() == "scope_changed");
    assert_eq!(changed.count(), 1, "only the move into the project changes the scope");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn an_empty_prompt_is_invalid_and_stays_refused_on_a_retry() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let empty = daemon.prompt(1, "   ", TTY);
    let first = client.call::<Value>(empty.clone()).await;
    let again = client.call::<Value>(empty).await;

    let (Err(ClientError::Server { body: first }), Err(ClientError::Server { body: again })) =
        (first, again)
    else {
        panic!("an empty prompt must be refused");
    };
    assert_eq!(first.code, ErrorCode::Invalid);
    assert_eq!(again, first);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_prompt_for_a_conversation_that_does_not_exist_is_not_found() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();

    let Method::PromptSend(params) = daemon.prompt(1, "hello", TTY) else { unreachable!() };
    let unknown = "0192f0c1-7a00-7000-8000-00000000dead".parse().unwrap();
    let method = Method::PromptSend(PromptSend { conversation_id: Some(unknown), ..params });
    let refused = client.call::<Value>(method).await;

    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    drop(client);
    daemon.stop().await.unwrap();
}
