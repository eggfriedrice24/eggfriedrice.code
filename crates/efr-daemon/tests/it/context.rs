//! `conversation.context` over a `TestDaemon`: the method answers with the parts of the
//! next request for a conversation whose actor is gone, needs only the `read` scope,
//! and refuses a conversation that does not exist.

use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{
    ContextPartKind, ConversationContext, ConversationContextResult, ConversationId, ErrorCode,
    Event, Method, PromptSendResult,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{ClientError, TTY, TestDaemon, events_until};
use pretty_assertions::assert_eq;

/// A model that answers every request with `ok` at once.
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

async fn start() -> TestDaemon {
    let model = Arc::new(Model { id: ProviderId::new("test").unwrap() });
    TestDaemon::builder()
        .custom_provider(model as Arc<dyn Provider>)
        .config(|config| config.jobs.max_wakes_per_hour = 0)
        .start()
        .await
        .unwrap()
}

fn context(conversation_id: ConversationId) -> Method {
    Method::ConversationContext(ConversationContext { conversation_id, model: None })
}

#[tokio::test]
async fn the_context_of_a_conversation_lists_its_parts() {
    let daemon = start().await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(1, "hello there", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    events_until(&mut follow, |e| matches!(e, Event::TurnCompleted { .. })).await.unwrap();

    let result: ConversationContextResult =
        client.call(context(sent.conversation_id)).await.unwrap();

    assert!(!result.running);
    let kinds: Vec<ContextPartKind> = result.parts.iter().map(|part| part.kind).collect();
    for kind in [ContextPartKind::System, ContextPartKind::Prompts, ContextPartKind::Preambles] {
        assert!(kinds.contains(&kind), "{kind:?} in {kinds:?}");
    }
    assert_eq!(result.estimate, result.parts.iter().map(|part| part.tokens).sum::<u64>());
    assert_eq!(result.compactions.count, 0);
}

#[tokio::test]
async fn an_unknown_conversation_is_not_found() {
    let daemon = start().await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let missing = ConversationId::from_uuid(uuid::Uuid::from_u128(77));

    let refused = client.call::<ConversationContextResult>(context(missing)).await;

    match refused {
        Err(ClientError::Server { body }) => assert_eq!(body.code, ErrorCode::NotFound, "{body:?}"),
        other => panic!("not refused as not found: {other:?}"),
    }
}
