//! `conversation.compact` over a `TestDaemon`: a manual compaction records
//! `conversation_compacted`, a retry answers from its receipt, the next turn sends the
//! summary, and the refusals carry their codes.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_protocol::{
    CompactionTrigger, ConversationCompact, ConversationCompactResult, ConversationId, ErrorCode,
    Event, Method, PromptSendResult, TurnId,
};
use efr_provider::{
    Message, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request,
    StopReason,
};
use efr_test_daemon::{Client, ClientError, TTY, TestDaemon, command_id, events_until};
use pretty_assertions::assert_eq;
use serde_json::Value;

/// What the model answers a summary request with.
const SUMMARY: &str = "## Task and state\nThe user pasted a long log.";

/// A model that answers each request at once: the summary to a summary request (its
/// last message asks efr's sections), `ok` to the rest. It keeps every request.
#[derive(Debug)]
struct Model {
    id: ProviderId,
    requests: Mutex<Vec<Request>>,
}

impl Model {
    fn new() -> Arc<Self> {
        Arc::new(Model { id: ProviderId::new("test").unwrap(), requests: Mutex::new(Vec::new()) })
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn is_summary_request(request: &Request) -> bool {
    request.messages.last().is_some_and(|message| message.text().contains("## Task and state"))
}

#[async_trait]
impl Provider for Model {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let text = if is_summary_request(&request) { SUMMARY } else { "ok" };
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        Ok(Box::pin(futures::stream::iter([
            Ok(ProviderEvent::TextDelta { text: text.to_owned() }),
            Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
        ])))
    }
}

async fn start(model: &Arc<Model>) -> (TestDaemon, Client) {
    let daemon = TestDaemon::builder()
        .custom_provider(Arc::clone(model) as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    (daemon, client)
}

/// Sends the prompt `text` and waits for the end of its turn.
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

fn compact(n: u128, conversation_id: ConversationId, focus: Option<&str>) -> Method {
    Method::ConversationCompact(ConversationCompact {
        command_id: command_id(n),
        conversation_id,
        focus: focus.map(str::to_owned),
    })
}

#[tokio::test]
async fn a_manual_compaction_is_recorded_answered_again_from_its_receipt_and_sent_next() {
    let model = Model::new();
    let (daemon, client) = start(&model).await;
    // NOTE: about 30000 tokens, more than the verbatim tail keeps, so the first turn
    // lies before the cut.
    let log = format!("look at this log\n{}", "error: disk full\n".repeat(7_000));
    let first = turn(&daemon, &client, 1, &log).await;
    let conversation_id = first.conversation_id;
    turn(&daemon, &client, 2, "what now?").await;

    let answer: Value = client.call(compact(3, conversation_id, Some("the disk"))).await.unwrap();
    let retried: Value = client.call(compact(3, conversation_id, Some("the disk"))).await.unwrap();

    assert_eq!(retried, answer, "a retry answers from the receipt");
    let result: ConversationCompactResult = serde_json::from_value(answer).unwrap();
    let compaction = &result.compaction;
    assert_eq!(compaction.trigger, CompactionTrigger::Manual);
    assert_eq!(compaction.turn_id, None);
    assert_eq!(compaction.focus.as_deref(), Some("the disk"));
    assert_eq!(compaction.through_turn, first.turn_id);
    assert_eq!(compaction.through_message, Some(1), "the tail keeps the first answer");
    assert_eq!(compaction.summary.as_deref(), Some(SUMMARY));
    assert!(compaction.tokens_after < compaction.tokens_before, "{compaction:?}");
    let summaries = model.requests().iter().filter(|r| is_summary_request(r)).count();
    assert_eq!(summaries, 1, "the retry ran nothing");
    let summary_request = model.requests().into_iter().find(is_summary_request).unwrap();
    let last = summary_request.messages.last().map(Message::text).unwrap_or_default();
    assert!(last.ends_with("Keep in the summary: the disk"), "{last}");
    let events = daemon.events(&client, conversation_id).await.unwrap();
    let recorded = events.iter().find(|e| e.seq == result.seq).map(|e| e.event.clone());
    assert_eq!(recorded, Some(Event::ConversationCompacted(compaction.clone())));
    assert!(
        !events.iter().any(|e| e.seq > result.seq && e.event.kind() == "turn_started"),
        "the compaction starts no turn"
    );

    turn(&daemon, &client, 4, "and then?").await;

    let next = model.requests().pop().unwrap();
    let texts: Vec<String> = next.messages.iter().map(Message::text).collect();
    assert!(texts[0].starts_with("<fresh-context>\n"), "{}", texts[0]);
    assert!(texts[1].starts_with("<conversation-summary>\n## Task and state"), "{}", texts[1]);
    assert_eq!(texts[2], "ok", "the tail starts after the long log");
    assert!(texts[3].starts_with("<live_state>\n"), "the prompt keeps its preamble: {}", texts[3]);
    assert!(texts[3].ends_with("</live_state>\n\nwhat now?"), "{}", texts[3]);
    assert!(!texts.iter().any(|text| text.contains("error: disk full")));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_short_conversation_has_nothing_to_compact_and_an_unknown_one_is_not_found() {
    let model = Model::new();
    let (daemon, client) = start(&model).await;
    let sent = turn(&daemon, &client, 1, "hello").await;

    let short: Result<Value, ClientError> =
        client.call(compact(2, sent.conversation_id, None)).await;
    let unknown: Result<Value, ClientError> =
        client.call(compact(3, ConversationId::from_uuid(*command_id(99).as_uuid()), None)).await;

    let code = |result: Result<Value, ClientError>| match result {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("a refusal: {other:?}"),
    };
    assert_eq!(code(short), ErrorCode::Conflict);
    assert_eq!(code(unknown), ErrorCode::NotFound);
    let events = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert!(!events.iter().any(|e| e.event.kind() == "conversation_compacted"));
    daemon.stop().await.unwrap();
}
