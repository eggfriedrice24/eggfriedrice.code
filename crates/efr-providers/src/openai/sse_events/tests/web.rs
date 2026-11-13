//! The hosted web tool in the stream. The fixtures `web_search*.sse` are written from
//! developers.openai.com/api/docs/guides/tools-web-search and
//! developers.openai.com/api/reference/resources/responses/streaming-events (read on
//! 2026-11-12); none is a recording.

use efr_provider::{
    CompletionBuilder, ContentBlock, HostedAction, HostedError, HostedOutcome, ProviderEvent,
    StopReason,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{done_items, map_all, sse, text, usage};
use crate::openai::testing::fixture_events;

const PAGE: &str = "https://areweanticheatyet.com/game/rob-run";

fn start(call_id: &str, action: HostedAction) -> ProviderEvent {
    ProviderEvent::HostedCallStart { call_id: call_id.to_owned(), action }
}

fn end(call_id: &str, action: HostedAction, outcome: HostedOutcome) -> ProviderEvent {
    ProviderEvent::HostedCallEnd { call_id: call_id.to_owned(), action, outcome }
}

fn done() -> HostedOutcome {
    HostedOutcome::Done { results: None }
}

fn failed(error: HostedError) -> HostedOutcome {
    HostedOutcome::Failed { error }
}

fn search(query: Option<&str>) -> HostedAction {
    HostedAction::Search { query: query.map(str::to_owned) }
}

fn cite(url: &str, title: Option<&str>) -> ProviderEvent {
    ProviderEvent::Citation { url: url.to_owned(), title: title.map(str::to_owned), placed: false }
}

#[test]
fn a_search_an_open_and_a_find_are_hosted_calls_and_the_citation_is_not_placed() {
    let events = fixture_events("web_search.sse");
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    let open = HostedAction::Open { url: Some(PAGE.to_owned()) };
    let find =
        HostedAction::Find { pattern: "Easy Anti-Cheat".to_owned(), url: Some(PAGE.to_owned()) };
    assert_eq!(
        mapped,
        vec![
            // The item of `output_item.added` has no action: the start has no query, the
            // end has it (TM-Web 6: the status row shows no detail until then).
            start("ws_1", search(None)),
            end("ws_1", search(Some("\"ROB RUN\" areweanticheatyet")), done()),
            start("ws_2", search(None)),
            end("ws_2", open, done()),
            start("ws_3", search(None)),
            end("ws_3", find, done()),
            text("The developers allowed Linux with patch 1.4 "),
            text(&format!("([areweanticheatyet.com]({PAGE})).")),
            cite(PAGE, Some("ROB RUN · Are We Anti-Cheat Yet?")),
            usage(9200, 310, 0, 0),
            ProviderEvent::Done {
                stop_reason: StopReason::EndTurn,
                provider_raw: Some(done_items(&events)),
            },
        ]
    );
}

#[test]
fn the_web_calls_are_borders_of_the_text_and_stay_verbatim_in_the_raw_items() {
    let events = fixture_events("web_search.sse");
    let (mapped, _) = map_all(&events);
    let mut builder = CompletionBuilder::new();
    for event in &mapped {
        builder.push(event).unwrap();
    }
    let completion = builder.finish().unwrap();
    // The canonical message holds the answer only, as the model wrote it, with its
    // Markdown link: DECISIONS-web 4a, the fallback for OpenAI.
    assert_eq!(
        completion.message.content,
        vec![ContentBlock::Text {
            text: format!(
                "The developers allowed Linux with patch 1.4 ([areweanticheatyet.com]({PAGE}))."
            ),
        }]
    );
    let raw = completion.message.provider_raw.unwrap();
    let kinds: Vec<&str> =
        raw.as_array().unwrap().iter().map(|item| item["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["web_search_call", "web_search_call", "web_search_call", "message"]);
    assert_eq!(raw, done_items(&events));
}

#[test]
fn a_failed_search_ends_as_not_available() {
    let events = fixture_events("web_search_failed.sse");
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped[..2],
        [
            start("ws_fail1", search(None)),
            end(
                "ws_fail1",
                search(Some("\"ROB RUN\" anti-cheat linux")),
                failed(HostedError::Unavailable)
            ),
        ]
    );
    assert_eq!(mapped[2], text("The search failed, so I answer from what I know."));
    assert!(matches!(
        mapped.last(),
        Some(ProviderEvent::Done { stop_reason: StopReason::EndTurn, .. })
    ));
}

#[test]
fn a_search_with_three_queries_is_three_searches_in_order() {
    let (mapped, error) = map_all(&fixture_events("web_search_queries.sse"));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped[..6],
        [
            start("ws_multi1", search(None)),
            end("ws_multi1", search(Some("\"ROB RUN\" anti-cheat")), done()),
            start("ws_multi1#1", search(Some("\"ROB RUN\" Proton"))),
            end("ws_multi1#1", search(Some("\"ROB RUN\" Proton")), done()),
            start("ws_multi1#2", search(Some("\"ROB RUN\" patch 1.4"))),
            end("ws_multi1#2", search(Some("\"ROB RUN\" patch 1.4")), done()),
        ]
    );
    assert_eq!(mapped[6], text("Three searches agree: patch 1.4 allowed Linux."));
}

#[test]
fn a_finished_item_without_an_action_is_a_search_without_a_query() {
    let (mapped, error) = map_all(&fixture_events("web_search_no_action.sse"));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped[..2],
        [start("ws_bare1", search(None)), end("ws_bare1", search(None), done())]
    );
}

#[test]
fn an_item_sent_only_whole_starts_and_ends_its_call() {
    let events = sse(&[
        json!({"type": "response.output_item.done", "output_index": 0, "item": {
            "id": "ws_1", "type": "web_search_call", "status": "completed",
            "action": {"type": "open_page", "url": PAGE}}}),
        json!({"type": "response.completed", "response": {}}),
    ]);
    let open = HostedAction::Open { url: Some(PAGE.to_owned()) };
    assert_eq!(map_all(&events).0[..2], [start("ws_1", open.clone()), end("ws_1", open, done())]);
}

#[test]
fn the_progress_events_start_a_call_whose_item_did_not_come() {
    let events = sse(&[
        json!({"type": "response.web_search_call.in_progress", "output_index": 0, "item_id": "ws_1"}),
        json!({"type": "response.web_search_call.searching", "output_index": 0, "item_id": "ws_1"}),
        json!({"type": "response.web_search_call.completed", "output_index": 0, "item_id": "ws_1"}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": {
            "id": "ws_1", "type": "web_search_call", "status": "completed",
            "action": {"type": "search", "query": "rob run"}}}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    // None of the three goes out as `Raw`.
    assert_eq!(
        mapped,
        vec![start("ws_1", search(None)), end("ws_1", search(Some("rob run")), done())]
    );
}

#[test]
fn a_call_whose_item_never_finished_ends_with_the_response() {
    let listed = json!({"id": "ws_2", "type": "web_search_call", "status": "completed",
        "action": {"type": "search", "query": "rob run"}});
    let events = sse(&[
        json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": "ws_1", "type": "web_search_call", "status": "in_progress"}}),
        json!({"type": "response.output_item.added", "output_index": 1, "item": {"id": "ws_2", "type": "web_search_call", "status": "in_progress"}}),
        json!({"type": "response.completed", "response": {"output": [listed]}}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped,
        vec![
            start("ws_1", search(None)),
            start("ws_2", search(None)),
            end("ws_1", search(None), failed(HostedError::NotFinished)),
            end("ws_2", search(Some("rob run")), done()),
            ProviderEvent::Done {
                stop_reason: StopReason::EndTurn,
                provider_raw: Some(json!([listed]))
            },
        ]
    );
}

#[test]
fn an_incomplete_item_or_response_ends_its_call_as_not_finished() {
    let events = sse(&[
        json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": "ws_1", "type": "web_search_call", "status": "in_progress"}}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": {"id": "ws_1", "type": "web_search_call", "status": "incomplete"}}),
        json!({"type": "response.output_item.added", "output_index": 1, "item": {"id": "ws_2", "type": "web_search_call", "status": "in_progress"}}),
        json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"}}}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped[..4],
        [
            start("ws_1", search(None)),
            end("ws_1", search(None), failed(HostedError::NotFinished)),
            start("ws_2", search(None)),
            end("ws_2", search(None), failed(HostedError::NotFinished)),
        ]
    );
    assert!(matches!(
        mapped.last(),
        Some(ProviderEvent::Done { stop_reason: StopReason::MaxTokens, .. })
    ));
}

#[test]
fn a_web_call_with_a_function_call_stops_for_the_function_call_only() {
    let events = sse(&[
        json!({"type": "response.output_item.done", "output_index": 0, "item": {"id": "ws_1", "type": "web_search_call", "status": "completed"}}),
        json!({"type": "response.output_item.done", "output_index": 1, "item": {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{}"}}),
        json!({"type": "response.completed", "response": {}}),
    ]);
    assert!(matches!(
        map_all(&events).0.last(),
        Some(ProviderEvent::Done { stop_reason: StopReason::ToolUse, .. })
    ));
}

fn message(id: &str, text: &str, annotations: serde_json::Value) -> serde_json::Value {
    json!({"id": id, "type": "message", "status": "completed", "role": "assistant",
        "content": [{"type": "output_text", "text": text, "annotations": annotations}]})
}

#[test]
fn the_citations_of_a_message_come_once_per_url_in_the_order_of_their_spans() {
    let events = sse(&[
        json!({"type": "response.output_item.done", "output_index": 0, "item": message("msg_1", "Three pages.", json!([
            {"type": "url_citation", "start_index": 30, "end_index": 40, "url": "https://www.protondb.com/app/2384710", "title": "ROB RUN · ProtonDB"},
            {"type": "file_citation", "file_id": "file-1", "index": 0, "filename": "a.txt"},
            {"type": "url_citation", "start_index": 2, "end_index": 9, "url": PAGE, "title": ""},
            {"type": "url_citation", "start_index": 50, "end_index": 60, "url": PAGE, "title": "again"},
        ]))}),
        json!({"type": "response.output_item.done", "output_index": 1, "item": message("msg_2", "More.", json!([
            {"type": "url_citation", "start_index": 0, "end_index": 4, "url": PAGE, "title": "ROB RUN"},
            {"type": "url_citation", "start_index": 1, "end_index": 4, "url": "https://store.steampowered.com/app/2384710/ROB_RUN/", "title": "ROB RUN on Steam"},
        ]))}),
        json!({"type": "response.completed", "response": {}}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped[..5],
        [
            text("Three pages."),
            cite(PAGE, None),
            cite("https://www.protondb.com/app/2384710", Some("ROB RUN · ProtonDB")),
            text("More."),
            cite("https://store.steampowered.com/app/2384710/ROB_RUN/", Some("ROB RUN on Steam")),
        ]
    );
}

#[test]
fn the_annotation_events_stand_in_when_the_finished_message_has_none() {
    let annotation = |index: u64, url: &str| {
        json!({"type": "response.output_text.annotation.added", "item_id": "msg_1", "output_index": 0,
            "content_index": 0, "annotation_index": index,
            "annotation": {"type": "url_citation", "start_index": 10 - index, "end_index": 12, "url": url, "title": "T"}})
    };
    let events = sse(&[
        json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 0, "delta": "Text."}),
        annotation(0, "https://a.example/one"),
        annotation(1, "https://a.example/two"),
        json!({"type": "response.output_item.done", "output_index": 0, "item": message("msg_1", "Text.", json!([]))}),
        json!({"type": "response.completed", "response": {}}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    // The annotation events are consumed: no `Raw`.
    assert_eq!(
        mapped[..3],
        [
            text("Text."),
            cite("https://a.example/two", Some("T")),
            cite("https://a.example/one", Some("T")),
        ]
    );
}
