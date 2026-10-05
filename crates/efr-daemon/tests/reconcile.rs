//! Restart reconciliation over a `TestDaemon` with a file store: what was in flight
//! when the daemon stopped is settled at the next start, and nothing continues on its
//! own.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{
    ConversationStatus, ConversationsList, ConversationsListResult, Event, Method, PromptSendResult,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{Replay, Scenario, TTY, TestDaemon, events_until};
use pretty_assertions::assert_eq;

#[tokio::test]
async fn restart_reconcile_inflight_turn() {
    let mut replay =
        Replay::start(Scenario::load("restart_reconcile_inflight_turn").unwrap()).await.unwrap();
    let before = replay.daemon().daemon_id();
    replay.run_to_end().await.unwrap();

    assert_eq!(replay.daemon().daemon_id(), before, "the daemon id survives a restart");
    let first = replay.bindings().get("<turn:1>").unwrap().to_owned();
    let second = replay.bindings().get("<turn:2>").unwrap().to_owned();
    let events = replay.events().await.unwrap();
    let tail: Vec<(String, String)> = events
        .iter()
        .rev()
        .take(2)
        .rev()
        .map(|envelope| {
            let turn = envelope.event.turn_id().map(|id| id.to_string()).unwrap_or_default();
            (envelope.event.kind().to_owned(), turn)
        })
        .collect();
    assert_eq!(
        tail,
        [("turn_cancelled".to_owned(), first), ("turn_cancelled".to_owned(), second.clone())],
        "the running turn is cancelled and the queued prompt recorded as not run, in that order"
    );
    let started = events.iter().filter(|e| e.event.kind() == "turn_started").count();
    assert_eq!(started, 1, "the queued prompt does not start by itself");
    let notice = replay.daemon().dirs().dirs().runtime().join("notices").join("pts-efr-test");
    assert_eq!(
        std::fs::read_to_string(&notice).unwrap(),
        "efr restarted; your queued prompt was not run: and then this; send it again\n",
        "the terminal is told at its next prompt"
    );
    let list: ConversationsListResult = replay
        .client()
        .call(Method::ConversationsList(ConversationsList::default()))
        .await
        .unwrap();
    assert_eq!(list.conversations[0].status, ConversationStatus::Idle);
    assert_eq!(replay.provider().unwrap().served(), 1);
    replay.stop().await.unwrap();
}

/// A model that answers every request with the same words.
#[derive(Debug)]
struct OneAnswer {
    id: ProviderId,
}

#[async_trait]
impl Provider for OneAnswer {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let events = vec![
            Ok(ProviderEvent::TextDelta { text: "noted".to_owned() }),
            Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

#[tokio::test]
async fn a_restart_keeps_the_log_and_each_terminals_conversation() {
    let provider = Arc::new(OneAnswer { id: ProviderId::new("test").unwrap() });
    let mut daemon =
        TestDaemon::builder().persistent().custom_provider(provider).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let first: PromptSendResult = client.call(daemon.prompt(1, "one", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, first.conversation_id).await.unwrap();
    events_until(&mut follow, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();
    drop((follow, client));

    daemon.restart().await.unwrap();

    let client = daemon.client_for_tty(TTY).await.unwrap();
    let second: PromptSendResult = client.call(daemon.prompt(2, "two", TTY)).await.unwrap();
    assert_eq!(second.conversation_id, first.conversation_id, "the terminal's conversation");
    assert!(!second.queued);
    let mut follow = daemon.follow(&client, second.conversation_id).await.unwrap();
    let mut completed = 0;
    let events = events_until(&mut follow, |event| {
        completed += usize::from(matches!(event, Event::TurnCompleted { .. }));
        completed == 2
    })
    .await
    .unwrap();
    let kinds: Vec<&str> = events.iter().map(|envelope| envelope.event.kind()).collect();
    assert!(!kinds.contains(&"turn_cancelled"), "a finished turn is left alone: {kinds:?}");
    assert_eq!(kinds.iter().filter(|kind| **kind == "conversation_created").count(), 1);
    drop((follow, client));
    daemon.stop().await.unwrap();
}
