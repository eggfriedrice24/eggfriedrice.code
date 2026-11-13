//! The provider's items of web search in the history and the compaction: they stay in
//! the next requests, a summary request carries the same hosted tools as the turn, the
//! estimate of the context counts them, and a pruning keeps them (the build spec of web
//! search, 5.3 item 7, and 6.2).

use std::sync::Arc;

use efr_protocol::{ConversationCompact, Event, Mode};
use efr_provider::{
    ContentBlock, HostedAction, HostedOutcome, HostedTool, Message, Provider, ProviderError,
    ProviderEvent, ProviderId, ProviderStream, Request, Role, StopReason, WebAccess, WebResult,
    WebSupport,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::compaction::{LeftOut, Placed, prune, summary_request};
use crate::context::message_tokens;
use crate::testing::{Recorder, Setup, big_text, done, request, result_message, tool_message};

/// The raw items of a Claude answer with one search: its call, its result with the
/// encrypted content that must go back unchanged, and the cited text.
fn search_items() -> Value {
    json!([
        { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
          "input": { "query": "rust 2026" } },
        { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
          "content": [{ "type": "web_search_result", "url": "https://example.com/rust",
                        "title": "Rust 2026", "encrypted_content": "EqgfCioIARgB".repeat(64),
                        "page_age": null }] },
        { "type": "text", "text": "Rust 2026 is out",
          "citations": [{ "type": "web_search_result_location", "url": "https://example.com/rust",
                          "title": "Rust 2026", "encrypted_index": "Eo8BCioIAhgB",
                          "cited_text": "Rust 2026 ..." }] }
    ])
}

fn live() -> Vec<HostedTool> {
    vec![HostedTool::WebSearch {
        access: WebAccess::Live,
        max_uses: Some(20),
        urls_from_tool_results: false,
    }]
}

#[test]
fn a_summary_request_carries_the_hosted_tools_of_its_base() {
    let base = Request { hosted: live(), ..request(Vec::new()) };
    let summary = summary_request(&base, vec![Message::user("hi")], None, LeftOut::default());
    assert_eq!(summary.hosted, live());
    assert!(summary.side_call);
}

#[test]
fn the_estimate_of_a_message_counts_its_raw_search_items() {
    let canonical = Message::assistant("Rust 2026 is out [[1]](<https://example.com/rust>).");
    let with_items = Message { provider_raw: Some(search_items()), ..canonical.clone() };
    let raw_len = serde_json::to_string(&search_items()).expect("json").len() as u64;
    assert!(
        message_tokens(&with_items) >= message_tokens(&canonical) + raw_len / 4,
        "{} against {}",
        message_tokens(&with_items),
        message_tokens(&canonical)
    );
}

/// A pruning stubs old tool output only: the provider's items of an answer with a
/// search stay as they came, so Claude reads the encrypted content again.
#[test]
fn a_pruned_window_keeps_the_raw_search_items() {
    let turn = efr_protocol::TurnId::from_uuid(efr_stdx::id::uuid_v7(
        &efr_test_support::TestClock::new(),
        &efr_test_support::TestRng::new(3),
    ));
    let answer = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text { text: "Rust 2026 is out.".to_owned() }],
        provider_raw: Some(search_items()),
    };
    let read = json!({ "path": "/home/u/big" });
    let mut messages = vec![Message::user("read the big file"), answer.clone()];
    for n in 0..4 {
        let id = format!("call_{n}");
        messages.push(tool_message(&id, "read_file", &read));
        messages.push(result_message(&id, &big_text(200_000), false));
    }
    messages.push(Message::user("and now?"));
    let placed: Vec<Placed> = messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| Placed { turn, index: index as u32, message })
        .collect();
    let pruning = prune(&placed);
    assert!(pruning.outputs > 0, "the old outputs are pruned");
    assert_eq!(pruning.placed[1].message, answer);
}

/// A model that searches once in every answer and keeps each request.
#[derive(Debug)]
struct Searching {
    inner: Arc<Recorder>,
}

#[async_trait::async_trait]
impl Provider for Searching {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }

    fn web_search(&self) -> WebSupport {
        WebSupport::Live
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        if request.side_call {
            return self.inner.stream(request).await;
        }
        let _ = self.inner.stream(request).await?;
        let events = vec![
            ProviderEvent::HostedCallStart {
                call_id: "srvtoolu_1".to_owned(),
                action: HostedAction::Search { query: Some("rust 2026".to_owned()) },
            },
            ProviderEvent::HostedCallEnd {
                call_id: "srvtoolu_1".to_owned(),
                action: HostedAction::Search { query: Some("rust 2026".to_owned()) },
                outcome: HostedOutcome::Done {
                    results: Some(vec![WebResult::new("https://example.com/rust", None)]),
                },
            },
            ProviderEvent::TextDelta { text: "Rust 2026 is out.".to_owned() },
            done(StopReason::EndTurn, Some(search_items())),
        ];
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// A manual compaction after turns with searches: its summary request carries the web
/// tool of the turns, and the history that it summarizes holds their items.
#[tokio::test]
async fn a_manual_compaction_sends_the_web_tool_and_the_search_items() {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    let recorder = Recorder::new(1);
    let model = Arc::new(Searching { inner: Arc::clone(&recorder) });
    let mut h = setup.start_model(model).await;
    for text in [
        "what changed in rust 2026".to_owned(),
        format!("and this\n{}", big_text(100_000)),
        "ok".to_owned(),
    ] {
        let sent = h.prompt(&text).await;
        let end = h.wait_end(sent.turn_id).await;
        assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    }
    let params = ConversationCompact {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        focus: None,
    };
    h.handle.compact(params).await.expect("compacted");

    let requests = recorder.requests();
    let turns: Vec<&Request> = requests.iter().filter(|request| !request.side_call).collect();
    assert!(turns.iter().all(|request| request.hosted == live()));
    assert!(
        turns[1].messages.iter().any(|message| message.provider_raw == Some(search_items())),
        "the next request sends the items of the search back"
    );
    let summary = requests.iter().find(|request| request.side_call).expect("a summary request");
    assert_eq!(summary.hosted, live());
    assert!(summary.messages.iter().any(|message| message.provider_raw == Some(search_items())));
}
