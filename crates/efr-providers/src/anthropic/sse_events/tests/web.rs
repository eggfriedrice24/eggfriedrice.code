//! The web tools in the stream: server tool calls, their results and errors, the
//! citations, the held punctuation and `pause_turn`. The fixtures are written from the
//! pages that each names in its first line; none is a recording.

use efr_provider::{
    ContentBlock, HostedAction, HostedError, HostedOutcome, ProviderEvent, StopReason, TokenUsage,
    WebResult,
};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::{done, map_all_with, sse, start, stop, text};
use crate::anthropic::testing::{LogText, fixture_completion_with, fixture_events};

const AWACY: &str = "https://areweanticheatyet.com/game/rob-run";
const AWACY_TITLE: &str = "ROB RUN · Are We Anti-Cheat Yet?";
const PROTONDB: &str = "https://www.protondb.com/app/2384710";
const PROTONDB_TITLE: &str = "ROB RUN · ProtonDB";
const DECK: &str = "https://www.steamdeck.com/verified/2384710";
const DECK_TITLE: &str = "Steam Deck Verified: ROB RUN";

/// The events of the fixture `name` to a request with the web tools.
fn web(name: &str) -> Vec<ProviderEvent> {
    let (events, error) = map_all_with(&fixture_events(name), true);
    assert!(error.is_none(), "{error:?}");
    events
}

fn search(query: &str) -> HostedAction {
    HostedAction::Search { query: Some(query.to_owned()) }
}

fn open(url: &str) -> HostedAction {
    HostedAction::Open { url: Some(url.to_owned()) }
}

fn started(call_id: &str, action: HostedAction) -> ProviderEvent {
    ProviderEvent::HostedCallStart { call_id: call_id.to_owned(), action }
}

fn ended(call_id: &str, action: HostedAction, outcome: HostedOutcome) -> ProviderEvent {
    ProviderEvent::HostedCallEnd { call_id: call_id.to_owned(), action, outcome }
}

fn found(results: &[(&str, &str)]) -> HostedOutcome {
    HostedOutcome::Done {
        results: Some(
            results
                .iter()
                .map(|(url, title)| WebResult::new(*url, Some((*title).to_owned())))
                .collect(),
        ),
    }
}

fn failed(error: HostedError) -> HostedOutcome {
    HostedOutcome::Failed { error }
}

fn cited(url: &str, title: &str) -> ProviderEvent {
    ProviderEvent::Citation { url: url.to_owned(), title: Some(title.to_owned()), placed: true }
}

fn stop_reason(events: &[ProviderEvent]) -> StopReason {
    match events.last() {
        Some(ProviderEvent::Done { stop_reason, .. }) => *stop_reason,
        other => panic!("{other:?}"),
    }
}

fn raw_of(events: &[ProviderEvent]) -> String {
    match events.last() {
        Some(ProviderEvent::Done { provider_raw: Some(Value::String(raw)), .. }) => raw.clone(),
        other => panic!("{other:?}"),
    }
}

fn hosted(events: &[ProviderEvent]) -> Vec<ProviderEvent> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                ProviderEvent::HostedCallStart { .. } | ProviderEvent::HostedCallEnd { .. }
            )
        })
        .cloned()
        .collect()
}

#[test]
fn a_search_streams_its_step_its_results_and_a_placed_citation_before_the_period() {
    let query = r#""ROB RUN" Proton Easy Anti-Cheat"#;
    assert_eq!(
        web("web_search.sse"),
        vec![
            text("I'll check what changed"),
            text("."),
            started("srvtoolu_01A", search(query)),
            ended(
                "srvtoolu_01A",
                search(query),
                found(&[(AWACY, AWACY_TITLE), (PROTONDB, PROTONDB_TITLE)])
            ),
            text("The developers allowed Linux in Easy Anti-Cheat with patch 1.4"),
            cited(AWACY, AWACY_TITLE),
            text(". Your setup forces Proton 9 for it"),
            text("."),
            ProviderEvent::Usage(TokenUsage {
                input_tokens: 10_682,
                output_tokens: 510,
                ..TokenUsage::default()
            }),
            done(
                StopReason::EndTurn,
                concat!(
                    r#"[{"type":"text","text":"I'll check what changed."},"#,
                    r#"{"type":"server_tool_use","id":"srvtoolu_01A","name":"web_search","input":{"query": "\"ROB RUN\" Proton Easy Anti-Cheat"}},"#,
                    r#"{"type":"web_search_tool_result","tool_use_id":"srvtoolu_01A","content":[{"type":"web_search_result","title":"ROB RUN · Are We Anti-Cheat Yet?","url":"https://areweanticheatyet.com/game/rob-run","encrypted_content":"Ev0DCioIAxgCIiQ3NmU4ZmI4OC1k","page_age":null},{"type":"web_search_result","title":"ROB RUN · ProtonDB","url":"https://www.protondb.com/app/2384710","encrypted_content":"EqgfCioIARgBIiQ3YTAwMjY1Mi1m","page_age":"April 30, 2026"}]},"#,
                    r#"{"type":"text","text":"The developers allowed Linux in Easy Anti-Cheat with patch 1.4","citations":[{"type":"web_search_result_location","url":"https://areweanticheatyet.com/game/rob-run","title":"ROB RUN · Are We Anti-Cheat Yet?","encrypted_index":"Eo8BCioIAhgBIiQyYjQ0OWJmZi1lNm","cited_text":"ROB RUN: supported since patch 1.4 ..."}]},"#,
                    r#"{"type":"text","text":". Your setup forces Proton 9 for it."}]"#,
                ),
            ),
        ]
    );
}

#[test]
fn without_the_web_tools_no_text_waits() {
    // A request without the web tools gets no search, but the stream keeps its order.
    let (events, error) = map_all_with(&fixture_events("web_search.sse"), false);
    assert!(error.is_none(), "{error:?}");
    let texts: Vec<&ProviderEvent> = events
        .iter()
        .filter(|event| {
            matches!(event, ProviderEvent::TextDelta { .. } | ProviderEvent::Citation { .. })
        })
        .collect();
    assert_eq!(
        texts,
        [
            &text("I'll check what changed."),
            &text("The developers allowed Linux in Easy Anti-Cheat with patch 1.4"),
            &cited(AWACY, AWACY_TITLE),
            &text(". Your setup forces Proton 9 for it."),
        ]
    );
}

#[rstest]
#[case::search("web_search.sse")]
#[case::error("web_search_error.sse")]
#[case::empty("web_search_empty.sse")]
#[case::pause("web_search_pause_turn.sse")]
#[case::continued("web_search_continued.sse")]
#[case::with_tool_use("web_search_with_tool_use.sse")]
#[case::fetch("web_fetch.sse")]
#[case::fetch_error("web_fetch_error.sse")]
#[case::three("web_search_three.sse")]
#[case::board("web_search_board.sse")]
#[case::long_query("web_search_long_query.sse")]
#[case::short("web_search_short.sse")]
fn every_held_character_comes_once_and_the_raw_text_never_changes(#[case] name: &str) {
    for web in [false, true] {
        let completion = fixture_completion_with(name, web);
        let Some(Value::String(raw)) = &completion.message.provider_raw else {
            panic!("{name}: no raw content");
        };
        let blocks: Vec<Value> = serde_json::from_str(raw).unwrap();
        let raw_text: String = blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .map(|block| block["text"].as_str().unwrap())
            .collect();
        let streamed: String = completion
            .message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(streamed, raw_text, "{name}, web {web}");
    }
}

#[test]
fn three_searches_at_once_end_in_their_order_and_cite_three_pages() {
    let events = web("web_search_three.sse");
    let protondb = r#""ROB RUN" Proton Easy Anti-Cheat"#;
    let awacy = r#""ROB RUN" areweanticheatyet"#;
    let starts = [
        started("srvtoolu_01T1", search(protondb)),
        started("srvtoolu_01T2", search(awacy)),
        started("srvtoolu_01T3", search("ROB RUN protondb")),
    ];
    let steps = hosted(&events);
    assert_eq!(steps[..3], starts);
    let counts: Vec<(String, usize)> = steps[3..]
        .iter()
        .map(|event| match event {
            ProviderEvent::HostedCallEnd {
                call_id,
                outcome: HostedOutcome::Done { results: Some(results) },
                ..
            } => (call_id.clone(), results.len()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        counts,
        [
            ("srvtoolu_01T1".to_owned(), 10),
            ("srvtoolu_01T2".to_owned(), 8),
            ("srvtoolu_01T3".to_owned(), 10)
        ]
    );
    // The answer of TM-Web 10: each citation stands right after the words that it
    // backs, and a period or a comma of the same block comes after it.
    let answer: Vec<ProviderEvent> = events
        .iter()
        .skip_while(|event| !matches!(event, ProviderEvent::TextDelta { .. }))
        .filter(|event| {
            matches!(event, ProviderEvent::TextDelta { .. } | ProviderEvent::Citation { .. })
        })
        .cloned()
        .collect();
    assert_eq!(
        answer,
        vec![
            text("The developers allowed Linux in Easy Anti-Cheat with patch 1.4"),
            cited(AWACY, AWACY_TITLE),
            text(". "),
            text("ProtonDB reports that it runs with Proton 10 and the Proton EasyAntiCheat"),
            text(" Runtime installed"),
            cited(PROTONDB, PROTONDB_TITLE),
            text(", and it is Verified on Steam Deck"),
            cited(DECK, DECK_TITLE),
            text("."),
            text(" Your setup forces Proton 9 for it, and the runtime is not installed"),
            text("."),
        ]
    );
}

#[test]
fn failed_searches_end_with_efrs_reasons_and_no_error_of_the_stream() {
    let events = web("web_search_error.sse");
    assert_eq!(stop_reason(&events), StopReason::EndTurn);
    let ends: Vec<ProviderEvent> = hosted(&events)
        .into_iter()
        .filter(|event| matches!(event, ProviderEvent::HostedCallEnd { .. }))
        .collect();
    assert_eq!(
        ends,
        vec![
            ended(
                "srvtoolu_01E1",
                search(r#""ROB RUN" 1.4 patch notes Linux"#),
                failed(HostedError::TooManyRequests)
            ),
            ended(
                "srvtoolu_01E2",
                search(r#""ROB RUN" kernel anti-cheat"#),
                failed(HostedError::LimitReached)
            ),
            ended("srvtoolu_01E3", search(r#""ROB RUN" Proton"#), failed(HostedError::Unavailable)),
        ]
    );
    // The error blocks go back as they came.
    assert!(raw_of(&events).contains(r#"{"type":"web_search_tool_result","tool_use_id":"srvtoolu_01E2","content":{"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"}}"#));
}

#[test]
fn a_search_without_matches_has_an_empty_list_of_results() {
    let query = r#""ROB RUN" EAC kernel 6.17"#;
    assert_eq!(
        hosted(&web("web_search_empty.sse")),
        vec![
            started("srvtoolu_01Z", search(query)),
            ended("srvtoolu_01Z", search(query), HostedOutcome::Done { results: Some(Vec::new()) }),
        ]
    );
}

#[test]
fn a_paused_answer_ends_with_a_call_that_has_not_run_and_logs_no_warning() {
    let log = LogText::default();
    let events =
        tracing::subscriber::with_default(log.subscriber(), || web("web_search_pause_turn.sse"));
    assert_eq!(stop_reason(&events), StopReason::PauseTurn);
    let steps = hosted(&events);
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert_eq!(steps[2], started("srvtoolu_01P2", search(r#""ROB RUN" Steam Deck verified"#)));
    let raw = raw_of(&events);
    assert!(
        raw.ends_with(r#"{"type":"server_tool_use","id":"srvtoolu_01P2","name":"web_search","input":{"query": "\"ROB RUN\" Steam Deck verified"}}]"#),
        "{raw}"
    );
    let text = log.text();
    assert!(!text.contains("WARN"), "{text}");
    assert!(text.contains("web_search_requests=1"), "{text}");
}

#[test]
fn a_continued_answer_ends_the_call_that_started_before_it() {
    assert_eq!(
        hosted(&web("web_search_continued.sse")),
        vec![ended(
            "srvtoolu_01P2",
            HostedAction::Search { query: None },
            found(&[(DECK, DECK_TITLE)])
        )]
    );
}

#[test]
fn a_search_next_to_a_client_call_waits_and_never_makes_a_tool_call() {
    let events = web("web_search_with_tool_use.sse");
    assert_eq!(stop_reason(&events), StopReason::ToolUse);
    assert_eq!(
        hosted(&events),
        vec![started("srvtoolu_01M", search(r#""ROB RUN" Proton Easy Anti-Cheat"#))]
    );
    let calls: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolCallStart { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["toolu_01M"]);
    let completion = fixture_completion_with("web_search_with_tool_use.sse", true);
    let canonical_calls = completion
        .message
        .content
        .iter()
        .filter(|block| matches!(block, ContentBlock::ToolCall { .. }))
        .count();
    assert_eq!(canonical_calls, 1);
}

#[test]
fn a_fetched_page_is_an_open_step() {
    assert_eq!(
        hosted(&web("web_fetch.sse")),
        vec![
            started("srvtoolu_01F", open(AWACY)),
            ended("srvtoolu_01F", open(AWACY), HostedOutcome::Done { results: None }),
        ]
    );
}

#[test]
fn a_failed_fetch_ends_with_efrs_reason() {
    let store = "https://store.steampowered.com/app/2384710/ROB_RUN/";
    assert_eq!(
        hosted(&web("web_fetch_error.sse")),
        vec![
            started("srvtoolu_01G1", open(store)),
            ended("srvtoolu_01G1", open(store), failed(HostedError::NotAllowed)),
            started("srvtoolu_01G2", open(PROTONDB)),
            ended("srvtoolu_01G2", open(PROTONDB), failed(HostedError::NotAccessible)),
        ]
    );
}

/// The `HostedCallEnd` of a result block of `kind` with `content`, with no start in the
/// answer.
fn end_of(kind: &str, content: Value) -> ProviderEvent {
    let mut events = vec![start()];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": kind, "tool_use_id": "srvtoolu_1", "content": content}}));
    events.push(json!({"type": "content_block_stop", "index": 0}));
    events.extend(stop("end_turn"));
    let (mapped, error) = map_all_with(&sse(&events), true);
    assert!(error.is_none(), "{error:?}");
    hosted(&mapped).pop().unwrap()
}

#[rstest]
#[case::too_many("too_many_requests", HostedError::TooManyRequests)]
#[case::limit("max_uses_exceeded", HostedError::LimitReached)]
#[case::unavailable("unavailable", HostedError::Unavailable)]
#[case::input("invalid_tool_input", HostedError::Invalid)]
#[case::long("query_too_long", HostedError::Invalid)]
#[case::large("request_too_large", HostedError::Invalid)]
#[case::new("a_new_code", HostedError::Other)]
fn each_search_error_code_has_its_reason(#[case] code: &str, #[case] error: HostedError) {
    assert_eq!(
        end_of(
            "web_search_tool_result",
            json!({"type": "web_search_tool_result_error", "error_code": code})
        ),
        ended("srvtoolu_1", HostedAction::Search { query: None }, failed(error))
    );
}

#[rstest]
#[case::input("invalid_tool_input", HostedError::Invalid)]
#[case::long("url_too_long", HostedError::Invalid)]
#[case::filtered("url_not_allowed", HostedError::NotAllowed)]
#[case::unseen("url_not_in_prior_context", HostedError::NotAllowed)]
#[case::unreachable("url_not_accessible", HostedError::NotAccessible)]
#[case::too_many("too_many_requests", HostedError::TooManyRequests)]
#[case::kind("unsupported_content_type", HostedError::Unsupported)]
#[case::large("content_too_large", HostedError::Unsupported)]
#[case::limit("max_uses_exceeded", HostedError::LimitReached)]
#[case::unavailable("unavailable", HostedError::Unavailable)]
#[case::new("a_new_code", HostedError::Other)]
fn each_fetch_error_code_has_its_reason(#[case] code: &str, #[case] error: HostedError) {
    assert_eq!(
        end_of(
            "web_fetch_tool_result",
            json!({"type": "web_fetch_tool_result_error", "error_code": code})
        ),
        ended("srvtoolu_1", HostedAction::Open { url: None }, failed(error))
    );
}

#[test]
fn a_result_without_its_start_tells_the_page_of_a_fetch() {
    assert_eq!(
        end_of("web_fetch_tool_result", json!({"type": "web_fetch_result", "url": AWACY})),
        ended("srvtoolu_1", open(AWACY), HostedOutcome::Done { results: None })
    );
}

/// The events of an answer with one `web_search` call whose start holds `start` and
/// whose deltas send `fragments`.
fn search_call(start_block: Value, fragments: &[&str]) -> Vec<ProviderEvent> {
    let mut events = vec![start()];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": start_block}));
    for fragment in fragments {
        events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": fragment}}));
    }
    events.push(json!({"type": "content_block_stop", "index": 0}));
    events.extend(stop("pause_turn"));
    let (mapped, error) = map_all_with(&sse(&events), true);
    assert!(error.is_none(), "{error:?}");
    mapped
}

#[rstest]
#[case::not_json(&[r#"{"query": "rob"#])]
#[case::not_an_object(&["[1]"])]
fn a_call_whose_input_is_not_an_object_loses_only_its_query(#[case] fragments: &[&str]) {
    let events = search_call(
        json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}}),
        fragments,
    );
    assert_eq!(hosted(&events), vec![started("srvtoolu_1", HostedAction::Search { query: None })]);
    assert_eq!(
        raw_of(&events),
        r#"[{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}]"#
    );
}

#[test]
fn a_start_without_input_and_without_deltas_has_an_empty_input() {
    // The streaming example of the web search page starts the block without `input`.
    let events = search_call(
        json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search"}),
        &[],
    );
    assert_eq!(hosted(&events), vec![started("srvtoolu_1", HostedAction::Search { query: None })]);
    assert_eq!(
        raw_of(&events),
        r#"[{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}]"#
    );
}

#[test]
fn a_start_with_its_input_and_no_deltas_keeps_that_input() {
    let events = search_call(
        json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "zsh"}}),
        &[],
    );
    assert_eq!(hosted(&events), vec![started("srvtoolu_1", search("zsh"))]);
}

/// The text and citation events of one text block that streams `fragments`, then
/// `citations`, to a request with the web tools when `web` is true.
fn text_block(fragments: &[&str], citations: &[Value], web: bool) -> Vec<ProviderEvent> {
    let mut events = vec![start()];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}));
    for fragment in fragments {
        events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": fragment}}));
    }
    for citation in citations {
        events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "citations_delta", "citation": citation}}));
    }
    events.push(json!({"type": "content_block_stop", "index": 0}));
    events.extend(stop("end_turn"));
    let (mapped, error) = map_all_with(&sse(&events), web);
    assert!(error.is_none(), "{error:?}");
    mapped
        .into_iter()
        .filter(|event| {
            matches!(event, ProviderEvent::TextDelta { .. } | ProviderEvent::Citation { .. })
        })
        .collect()
}

fn location(url: &str) -> Value {
    json!({"type": "web_search_result_location", "url": url, "title": "T", "encrypted_index": "x", "cited_text": "y"})
}

#[test]
fn punctuation_inside_a_block_goes_with_the_next_text() {
    assert_eq!(
        text_block(&["It runs. ", "On Linux, ", "too!? "], &[location(AWACY)], true),
        vec![
            text("It runs"),
            text(". On Linux"),
            text(", too"),
            ProviderEvent::Citation {
                url: AWACY.to_owned(),
                title: Some("T".to_owned()),
                placed: true
            },
            text("!? "),
        ]
    );
}

#[test]
fn a_block_of_only_punctuation_sends_it_at_its_stop() {
    assert_eq!(text_block(&[". ", "\n\n"], &[], true), vec![text(". \n\n")]);
}

#[test]
fn two_citations_of_one_block_come_in_order() {
    assert_eq!(
        text_block(&["Both say so."], &[location(AWACY), location(PROTONDB)], true),
        vec![
            text("Both say so"),
            ProviderEvent::Citation {
                url: AWACY.to_owned(),
                title: Some("T".to_owned()),
                placed: true
            },
            ProviderEvent::Citation {
                url: PROTONDB.to_owned(),
                title: Some("T".to_owned()),
                placed: true
            },
            text("."),
        ]
    );
}

#[test]
fn a_citation_of_another_type_gives_no_event_but_goes_back() {
    let char_location = json!({"type": "char_location", "document_index": 0, "document_title": "A", "start_char_index": 1, "end_char_index": 9, "cited_text": "z"});
    assert_eq!(
        text_block(&["From the page."], std::slice::from_ref(&char_location), true),
        vec![text("From the page"), text(".")]
    );
    let mut events = vec![start()];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}));
    events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hi."}}));
    events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "citations_delta", "citation": char_location}}));
    events.push(json!({"type": "content_block_stop", "index": 0}));
    events.extend(stop("end_turn"));
    let (mapped, _) = map_all_with(&sse(&events), true);
    assert!(raw_of(&mapped).contains(r#""citations":[{"#), "{mapped:?}");
}

#[test]
fn held_text_comes_before_the_end_of_a_block_that_never_stopped() {
    let mut events = vec![start()];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}));
    events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Cut here. "}}));
    events.extend(stop("max_tokens"));
    let (mapped, error) = map_all_with(&sse(&events), true);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(mapped[..2], [text("Cut here"), text(". ")]);
}

#[test]
fn held_text_is_taken_once_after_an_error() {
    let mut mapper = super::EventMapper::new().with_web_tools(true);
    let events = sse(&[
        start(),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Almost done. "}}),
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    ]);
    let mut out = Vec::new();
    for event in &events {
        if let Ok(mapped) = mapper.map(event) {
            out.extend(mapped);
        }
    }
    out.extend(mapper.take_held());
    assert_eq!(out, vec![text("Almost done"), text(". ")]);
    assert_eq!(mapper.take_held(), Vec::new());
}
