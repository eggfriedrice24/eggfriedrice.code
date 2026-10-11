//! `turn.undo` over a `TestDaemon`: the handler resolves the conversation of the
//! terminal, refuses a phone itself, and records nothing for a refusal or a dry run.
//! What an undo does with the files is in `tools/undo` and `tests/it/undo.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{
    ConversationId, ErrorBody, ErrorCode, Event, Method, Origin, PromptSendResult, TurnId,
    TurnUndo, TurnUndoResult,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{Client, ClientError, TTY, TestDaemon, command_id, events_until};
use pretty_assertions::assert_eq;
use serde_json::Value;

/// A model that answers each request with `ok` at once.
#[derive(Debug)]
struct Model {
    id: ProviderId,
}

#[async_trait]
impl Provider for Model {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures::stream::iter([
            Ok(ProviderEvent::TextDelta { text: "ok".to_owned() }),
            Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
        ])))
    }
}

async fn start() -> (TestDaemon, Client) {
    let model = Arc::new(Model { id: ProviderId::new("test").unwrap() });
    let daemon =
        TestDaemon::builder().custom_provider(model as Arc<dyn Provider>).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    (daemon, client)
}

/// Sends the prompt `text` from [`TTY`] and waits for the end of its turn.
async fn turn(daemon: &TestDaemon, client: &Client, n: u128, text: &str) -> PromptSendResult {
    let sent: PromptSendResult = client.call(daemon.prompt(n, text, TTY)).await.unwrap();
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
    let turn_id: TurnId = sent.turn_id;
    events_until(
        &mut follow,
        |e| matches!(e, Event::TurnCompleted { turn_id: t, .. } if *t == turn_id),
    )
    .await
    .unwrap();
    sent
}

fn undo(n: u128, conversation_id: Option<ConversationId>, turn_id: Option<TurnId>) -> Method {
    Method::TurnUndo(TurnUndo {
        command_id: command_id(n),
        conversation_id,
        turn_id,
        dry_run: true,
        force: false,
        redo: false,
        list: false,
    })
}

fn refused(result: Result<Value, ClientError>) -> ErrorBody {
    match result {
        Err(ClientError::Server { body }) => body,
        other => panic!("a refusal: {other:?}"),
    }
}

/// The answer of the actor of `conversation_id`: a preview of its newest turn, or a
/// refusal of the undo itself (`conflict` with the refusal), never a conversation that
/// the handler could not find.
fn reached_the_actor(result: Result<Value, ClientError>, conversation_id: ConversationId) {
    match result {
        Ok(answer) => {
            let answer: TurnUndoResult = serde_json::from_value(answer).unwrap();
            assert_eq!(answer.conversation_id, conversation_id);
            assert!(!answer.applied, "a dry run applies nothing");
        }
        Err(ClientError::Server { body }) => {
            assert_eq!(body.code, ErrorCode::Conflict, "{body:?}");
            let data = body.data.unwrap_or_default();
            assert!(data.get("refused").is_some(), "{data}");
        }
        Err(error) => panic!("{error:?}"),
    }
}

#[tokio::test]
async fn turn_undo_resolves_terminal_conversation() {
    let (daemon, client) = start().await;
    let sent = turn(&daemon, &client, 1, "write a note").await;

    reached_the_actor(client.call(undo(2, None, None)).await, sent.conversation_id);
    let other = daemon.client_for_tty("/dev/pts/efr-other").await.unwrap();
    reached_the_actor(other.call(undo(3, None, Some(sent.turn_id))).await, sent.conversation_id);
    let none = refused(other.call(undo(4, None, None)).await);
    assert_eq!(none.code, ErrorCode::NotFound, "another terminal has no conversation");

    let events = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert!(!events.iter().any(|e| e.event.kind() == "undo_applied"));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_phone_cannot_undo_and_nothing_is_recorded() {
    let (daemon, client) = start().await;
    let sent = turn(&daemon, &client, 1, "write a note").await;
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();

    let body = refused(phone.call(undo(2, Some(sent.conversation_id), None)).await);

    assert_eq!(body.code, ErrorCode::Forbidden);
    assert_eq!(body.data, Some(serde_json::json!({ "refused": { "kind": "phone_origin" } })));
    let events = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert!(!events.iter().any(|e| e.event.kind() == "undo_applied"));
    drop(phone);
    daemon.stop().await.unwrap();
}
