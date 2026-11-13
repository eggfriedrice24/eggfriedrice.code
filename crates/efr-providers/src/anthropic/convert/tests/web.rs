//! The web tools in the request: the two server tools, the replay of the blocks of a
//! search only in a request that carries them, the guard for a server tool call that
//! can no longer end, and the continuation of a paused turn.

use efr_provider::{ContentBlock, HostedTool, Message, Request, Role, WebAccess};
use pretty_assertions::assert_eq;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use super::{
    THINKING_BINDING_BETA, UNANSWERED_CALL, body, body_with, request, result, text, user, value,
};
use crate::anthropic::testing::fixture_completion_with;
use crate::anthropic::{AnthropicConfig, CacheTtl};

const ONE_HOUR: &str = r#","cache_control":{"type":"ephemeral","ttl":"1h"}"#;

/// `request` with the web tool of the turn: the live web, at most 20 searches.
fn with_web(mut request: Request) -> Request {
    request.hosted = vec![HostedTool::web_search(WebAccess::Live, Some(20))];
    request
}

/// The answer of the stream fixture `name` to a request with the web tools, as the
/// conversation stores it.
fn answer(name: &str) -> Message {
    let message = fixture_completion_with(name, true).message;
    assert!(message.provider_raw.is_some(), "{name}");
    message
}

fn raw(message: &Message) -> &str {
    match &message.provider_raw {
        Some(Value::String(raw)) => raw,
        other => panic!("{other:?}"),
    }
}

fn messages(request: &Request) -> Vec<Value> {
    value(&body(request))["messages"].as_array().unwrap().clone()
}

fn kinds(message: &Value) -> Vec<String> {
    message["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["type"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_web_tool_adds_web_search_and_web_fetch_after_the_client_tools() {
    let request = with_web(request(vec![Message::user("Does ROB RUN run on Linux?")]));
    let body = value(&body(&request));
    let tools = body["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["shell", "edit", "web_search", "web_fetch"]);
    assert_eq!(
        tools[2],
        json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 20})
    );
    assert_eq!(
        tools[3],
        json!({
            "type": "web_fetch_20250910",
            "name": "web_fetch",
            "max_uses": 20,
            "max_content_tokens": 25_000,
            "url_sources": {"client_tool_results": {"type": "none"}},
        })
    );
    assert_eq!(body["tool_choice"], json!({"type": "auto"}));
    // No new beta: both versions are plain tools of the API.
    assert_eq!(super::body(&request).betas(), &[THINKING_BINDING_BETA]);
    assert!(super::body(&request).carries_web_tools());
}

#[test]
fn full_mode_lets_a_fetch_take_urls_of_tool_results() {
    let mut request = request(vec![Message::user("Go.")]);
    request.hosted = vec![HostedTool::WebSearch {
        access: WebAccess::Stored,
        max_uses: None,
        urls_from_tool_results: true,
    }];
    let tools = value(&body(&request))["tools"].clone();
    // The API has only the live web, so the stored copy asks for the same tools.
    assert_eq!(tools[2], json!({"type": "web_search_20250305", "name": "web_search"}));
    assert_eq!(
        tools[3],
        json!({"type": "web_fetch_20250910", "name": "web_fetch", "max_content_tokens": 25_000})
    );
}

#[test]
fn a_request_without_the_web_tool_has_no_server_tool() {
    // Manual mode and `web.search = "off"` send no hosted tool: the provider adds none.
    let request = request(vec![Message::user("Does ROB RUN run on Linux?")]);
    let text = serde_json::to_string(&body(&request)).unwrap();
    assert!(!text.contains("web_search"), "{text}");
    assert!(!text.contains("web_fetch"), "{text}");
    assert!(!body(&request).carries_web_tools());
}

#[test]
fn the_marker_of_the_tools_stays_on_the_last_client_tool() {
    let mut request = with_web(request(vec![Message::user("Go.")]));
    request.system = None;
    let tools = value(&body(&request))["tools"].clone();
    assert_eq!(tools[1]["cache_control"], json!({"type": "ephemeral", "ttl": "1h"}));
    assert_eq!(tools[2].get("cache_control"), None);
    assert_eq!(tools[3].get("cache_control"), None);
}

#[test]
fn only_the_web_tools_still_get_a_tool_choice_and_no_marker() {
    let mut request = with_web(request(vec![Message::user("Go.")]));
    request.system = None;
    request.tools = Vec::new();
    let body = value(&body(&request));
    let text = body["tools"].to_string();
    assert!(!text.contains("cache_control"), "{text}");
    assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    assert_eq!(body["tool_choice"], json!({"type": "auto"}));
}

#[test]
fn the_whole_message_of_a_search_goes_back_byte_for_byte() {
    let searched = answer("web_search.sse");
    let request = with_web(request(vec![
        Message::user("Does ROB RUN run on Linux?"),
        searched.clone(),
        Message::user("And on Steam Deck?"),
    ]));
    let text = serde_json::to_string(&body(&request)).unwrap();
    let expected = format!(r#"{{"role":"assistant","content":{}}}"#, raw(&searched));
    assert!(text.contains(&expected), "{text}");
    // The encrypted content and the encrypted index go back as they came.
    assert!(text.contains(r#""encrypted_content":"Ev0DCioIAxgCIiQ3NmU4ZmI4OC1k""#));
    assert!(text.contains(r#""encrypted_index":"Eo8BCioIAhgBIiQyYjQ0OWJmZi1lNm""#));
}

#[test]
fn without_the_web_tool_a_message_of_a_search_goes_as_its_canonical_content() {
    // A later turn in manual mode: the search blocks stay out, the text stays.
    let searched = answer("web_search.sse");
    let request = request(vec![
        Message::user("Does ROB RUN run on Linux?"),
        searched.clone(),
        Message::user("Thanks."),
    ]);
    let messages = messages(&request);
    assert!(kinds(&messages[1]).iter().all(|kind| kind == "text"), "{messages:?}");
    let sent: Vec<&str> = messages[1]["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["text"].as_str().unwrap())
        .collect();
    let canonical: Vec<&str> = searched
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, canonical);
    let text = serde_json::to_string(&messages).unwrap();
    assert!(!text.contains("encrypted"), "{text}");
    assert!(!text.contains("citations"), "{text}");
}

#[test]
fn without_the_web_tool_a_cited_text_alone_also_falls_back() {
    let cited = r#"[{"type":"text","text":"It runs.","citations":[{"type":"web_search_result_location","url":"https://areweanticheatyet.com/game/rob-run","title":"t","encrypted_index":"e","cited_text":"c"}]}]"#;
    let message = Message::new(Role::Assistant, vec![text("It runs.")])
        .with_provider_raw(Value::String(cited.to_owned()));
    let plain = request(vec![Message::user("Go."), message.clone(), Message::user("More.")]);
    assert_eq!(
        messages(&plain)[1],
        json!({"role": "assistant", "content": [{"type": "text", "text": "It runs."}]})
    );
    let web = with_web(request(vec![Message::user("Go."), message, Message::user("More.")]));
    let text = serde_json::to_string(&body(&web)).unwrap();
    assert!(text.contains(&format!(r#"{{"role":"assistant","content":{cited}}}"#)), "{text}");
}

#[test]
fn the_continuation_of_a_paused_turn_ends_with_the_paused_answer() {
    let paused = answer("web_search_pause_turn.sse");
    let request =
        with_web(request(vec![Message::user("Does ROB RUN run on Linux?"), paused.clone()]));
    let text = serde_json::to_string(&body(&request)).unwrap();
    // The paused answer goes back as it came, its waiting call too, and the marker of
    // the tail stays on the prompt, where the paused request put it.
    let expected = format!(
        r#"{{"role":"user","content":[{{"type":"text","text":"Does ROB RUN run on Linux?"{ONE_HOUR}}}]}},{{"role":"assistant","content":{}}}]"#,
        raw(&paused)
    );
    assert!(text.ends_with(&format!("{expected}}}")), "{text}");
    // A waiting server tool call gets no tool result.
    assert!(!text.contains(UNANSWERED_CALL), "{text}");
}

#[test]
fn a_paused_answer_and_its_continuation_merge_into_one_assistant_message() {
    let paused = answer("web_search_pause_turn.sse");
    let continued = answer("web_search_continued.sse");
    let request = with_web(request(vec![
        Message::user("Does ROB RUN run on Linux?"),
        paused.clone(),
        continued.clone(),
        Message::user("Thanks."),
    ]));
    let messages = messages(&request);
    assert_eq!(messages.len(), 3);
    assert_eq!(
        kinds(&messages[1]),
        [
            "text",
            "server_tool_use",
            "web_search_tool_result",
            "server_tool_use",
            "web_search_tool_result",
            "text"
        ]
    );
    // Each block stays exact: the two contents, joined.
    let text = serde_json::to_string(&body(&request)).unwrap();
    let paused_blocks = raw(&paused).strip_suffix(']').unwrap();
    let continued_blocks = raw(&continued).strip_prefix('[').unwrap();
    let expected =
        format!(r#"{{"role":"assistant","content":{paused_blocks},{continued_blocks}}}"#);
    assert!(text.contains(&expected), "{text}");
}

#[test]
fn a_search_next_to_a_client_call_waits_for_the_tool_results() {
    let mixed = answer("web_search_with_tool_use.sse");
    let results = user(vec![result("toolu_01M", "12:EasyAntiCheat: platform not allowed")]);
    // The request after the client call: the server call stays, it runs now.
    let request = with_web(request(vec![Message::user("Go."), mixed.clone(), results.clone()]));
    let messages = messages(&request);
    assert_eq!(kinds(&messages[1]), ["text", "server_tool_use", "tool_use"]);
    assert_eq!(kinds(&messages[2]), ["tool_result"]);
    // The next request: the answer that starts with the result of the search follows.
    let resumed = Message::new(Role::Assistant, vec![text("It fails.")]).with_provider_raw(
        Value::String(
            r#"[{"type":"web_search_tool_result","tool_use_id":"srvtoolu_01M","content":[]},{"type":"text","text":"It fails."}]"#
                .to_owned(),
        ),
    );
    let request = with_web(request_of(vec![
        Message::user("Go."),
        mixed,
        results,
        resumed,
        Message::user("Why?"),
    ]));
    let messages = self::messages(&request);
    assert_eq!(kinds(&messages[1]), ["text", "server_tool_use", "tool_use"]);
}

fn request_of(messages: Vec<Message>) -> Request {
    request(messages)
}

#[test]
fn a_server_call_that_can_no_longer_end_is_left_out() {
    // A turn that an interrupt ended after the client call, and a new prompt.
    let mixed = answer("web_search_with_tool_use.sse");
    let request = with_web(request(vec![
        Message::user("Go."),
        mixed.clone(),
        user(vec![result("toolu_01M", "stopped")]),
        Message::user("Something else."),
    ]));
    let text = serde_json::to_string(&body(&request)).unwrap();
    let messages = messages(&request);
    assert_eq!(kinds(&messages[1]), ["text", "tool_use"]);
    // The other blocks go one by one, each exact.
    let blocks: Vec<Box<RawValue>> = serde_json::from_str(raw(&mixed)).unwrap();
    let kept: Vec<&str> = blocks
        .iter()
        .map(|block| block.get())
        .filter(|block| !block.contains("srvtoolu"))
        .collect();
    assert_eq!(kept.len(), 2);
    for block in kept {
        assert!(text.contains(block), "{block}");
    }
    assert!(!text.contains("srvtoolu_01M"), "{text}");
}

#[test]
fn a_paused_answer_that_never_went_on_loses_its_waiting_call() {
    // The turn failed after the pause; the next prompt follows the paused answer.
    let paused = answer("web_search_pause_turn.sse");
    let request =
        with_web(request(vec![Message::user("Go."), paused, Message::user("Something else.")]));
    let messages = messages(&request);
    assert_eq!(kinds(&messages[1]), ["text", "server_tool_use", "web_search_tool_result"]);
    let text = serde_json::to_string(&messages).unwrap();
    assert!(text.contains("srvtoolu_01P1"), "{text}");
    assert!(!text.contains("srvtoolu_01P2"), "{text}");
}

#[test]
fn a_message_of_only_a_call_that_can_no_longer_end_is_left_out() {
    let only = Message::new(Role::Assistant, Vec::new()).with_provider_raw(Value::String(
        r#"[{"type":"server_tool_use","id":"srvtoolu_9","name":"web_search","input":{"query": "x"}}]"#
            .to_owned(),
    ));
    let request = with_web(request(vec![Message::user("Go."), only, Message::user("Again.")]));
    let messages = messages(&request);
    let roles: Vec<&Value> = messages.iter().map(|message| &message["role"]).collect();
    assert_eq!(roles, ["user", "user"]);
}

#[test]
fn the_one_hour_markers_of_a_continuation_match_the_paused_request() {
    let paused = answer("web_search_pause_turn.sse");
    let config = AnthropicConfig::new().with_cache_ttl(CacheTtl::Auto);
    let before = with_web(request(vec![Message::user("Go.")]));
    let after = with_web(request(vec![Message::user("Go."), paused]));
    let marks = |request: &Request| {
        serde_json::to_string(&body_with(request, &config)).unwrap().matches(ONE_HOUR).count()
    };
    assert_eq!(marks(&before), marks(&after));
}
