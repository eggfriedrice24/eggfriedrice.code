use efr_http::{SseDecoder, SseEvent};
use efr_provider::{ContentBlock, ProviderError, ProviderEvent, StopReason, TokenUsage};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::EventMapper;
use crate::testing::{fixture_completion, fixture_events};

/// The events of a stream made of `events`, each sent under its own `type` as the event
/// name, the way the Messages API frames them.
fn sse(events: &[Value]) -> Vec<SseEvent> {
    let mut text = String::new();
    for event in events {
        let kind = event["type"].as_str().unwrap_or("message");
        text.push_str(&format!("event: {kind}\ndata: {event}\n\n"));
    }
    SseDecoder::new().push(text.as_bytes()).unwrap()
}

/// Maps `events` in order and returns the canonical events and the error that ended
/// the stream, if one did.
fn map_all(events: &[SseEvent]) -> (Vec<ProviderEvent>, Option<ProviderError>) {
    let mut mapper = EventMapper::new();
    let mut out = Vec::new();
    for event in events {
        match mapper.map(event) {
            Ok(mapped) => out.extend(mapped),
            Err(error) => {
                assert!(mapper.is_done());
                return (out, Some(error));
            }
        }
    }
    (out, None)
}

fn mapped(name: &str) -> Vec<ProviderEvent> {
    let (events, error) = map_all(&fixture_events(name));
    assert!(error.is_none(), "{error:?}");
    events
}

fn text(text: &str) -> ProviderEvent {
    ProviderEvent::TextDelta { text: text.to_owned() }
}

fn reasoning(text: &str) -> ProviderEvent {
    ProviderEvent::ReasoningDelta { text: text.to_owned() }
}

fn done(stop_reason: StopReason, raw: &str) -> ProviderEvent {
    ProviderEvent::Done { stop_reason, provider_raw: Some(Value::String(raw.to_owned())) }
}

fn start() -> Value {
    json!({"type": "message_start", "message": {
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [], "stop_reason": null,
        "usage": {"input_tokens": 5, "output_tokens": 1},
    }})
}

fn text_block(index: u64, text: &str) -> [Value; 3] {
    [
        json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": text}}),
        json!({"type": "content_block_stop", "index": index}),
    ]
}

fn stop(reason: &str) -> [Value; 2] {
    [
        json!({"type": "message_delta", "delta": {"stop_reason": reason}, "usage": {"output_tokens": 7}}),
        json!({"type": "message_stop"}),
    ]
}

#[test]
fn text_streams_deltas_then_usage_then_done() {
    assert_eq!(
        mapped("text.sse"),
        vec![
            text("Your shell"),
            text(" is zsh"),
            text(" 5.9."),
            ProviderEvent::Usage(TokenUsage {
                input_tokens: 1_224,
                output_tokens: 11,
                cached_input_tokens: 0,
                reasoning_tokens: 0,
                cache_write_tokens: 1_210,
                cache_write_1h_tokens: 1_210,
            }),
            done(StopReason::EndTurn, r#"[{"type":"text","text":"Your shell is zsh 5.9."}]"#),
        ]
    );
}

#[test]
fn a_tool_call_starts_grows_and_ends_with_the_input_that_the_model_wrote() {
    let call_id = "toolu_01T1x5fJ9mB7sQe3RkVw8ZpN".to_owned();
    assert_eq!(
        mapped("tool_use.sse"),
        vec![
            reasoning("The user wants the files."),
            text("I list them."),
            ProviderEvent::ToolCallStart {
                call_id: call_id.clone(),
                name: "shell".to_owned(),
                freeform: false,
            },
            // The empty first fragment sends nothing.
            ProviderEvent::ToolCallDelta {
                call_id: call_id.clone(),
                arguments: r#"{"command": "ls"#.to_owned(),
            },
            ProviderEvent::ToolCallDelta {
                call_id: call_id.clone(),
                arguments: r#" -a", "cwd": null}"#.to_owned(),
            },
            ProviderEvent::ToolCallEnd {
                call_id,
                arguments: r#"{"command": "ls -a", "cwd": null}"#.to_owned(),
            },
            ProviderEvent::Usage(TokenUsage {
                input_tokens: 12_256,
                output_tokens: 95,
                cached_input_tokens: 11_800,
                reasoning_tokens: 41,
                cache_write_tokens: 420,
                cache_write_1h_tokens: 0,
            }),
            done(
                StopReason::ToolUse,
                concat!(
                    r#"[{"type":"thinking","thinking":"The user wants the files.","signature":"EqQBCkgIBRABGAIiQL0kNdpb3Q=="},"#,
                    r#"{"type":"text","text":"I list them."},"#,
                    r#"{"type":"tool_use","id":"toolu_01T1x5fJ9mB7sQe3RkVw8ZpN","name":"shell","input":{"command": "ls -a", "cwd": null}}]"#,
                ),
            ),
        ]
    );
}

#[test]
fn a_tool_call_folds_into_the_canonical_message() {
    let completion = fixture_completion("tool_use.sse");
    assert_eq!(completion.stop_reason, StopReason::ToolUse);
    assert_eq!(
        completion.message.content,
        vec![
            ContentBlock::Reasoning { text: "The user wants the files.".to_owned() },
            ContentBlock::Text { text: "I list them.".to_owned() },
            ContentBlock::ToolCall {
                call_id: "toolu_01T1x5fJ9mB7sQe3RkVw8ZpN".to_owned(),
                name: "shell".to_owned(),
                input: json!({"command": "ls -a", "cwd": null}),
                freeform: false,
            },
        ]
    );
    assert!(completion.message.provider_raw.unwrap().is_string());
}

#[test]
fn two_thinking_blocks_read_as_two_sections_and_keep_their_signatures() {
    assert_eq!(
        mapped("thinking.sse"),
        vec![
            reasoning("I read the error"),
            reasoning(" first."),
            reasoning("\n\nThen the fix."),
            text("The build fails on a missing import."),
            ProviderEvent::Usage(TokenUsage {
                input_tokens: 24_103,
                output_tokens: 120,
                cached_input_tokens: 24_100,
                reasoning_tokens: 88,
                ..TokenUsage::default()
            }),
            done(
                StopReason::EndTurn,
                concat!(
                    r#"[{"type":"thinking","thinking":"I read the error first.","signature":"Ep4BCkYIBhgCKkB1c2lnbmF0dXJlLW9uZQ=="},"#,
                    r#"{"type":"thinking","thinking":"Then the fix.","signature":"Ep4BCkYIBhgCKkB1c2lnbmF0dXJlLXR3bw=="},"#,
                    r#"{"type":"text","text":"The build fails on a missing import."}]"#,
                ),
            ),
        ]
    );
}

#[test]
fn omitted_thinking_sends_no_reasoning_and_keeps_the_empty_block() {
    let events = mapped("thinking_omitted.sse");
    assert!(!events.iter().any(|event| matches!(event, ProviderEvent::ReasoningDelta { .. })));
    assert_eq!(
        events.last(),
        Some(&done(
            StopReason::EndTurn,
            r#"[{"type":"thinking","thinking":"","signature":"EpQBCkYIBhgCKkBvbWl0dGVk"},{"type":"text","text":"Done."}]"#,
        ))
    );
}

#[test]
fn a_redacted_block_goes_into_the_raw_content_as_it_came() {
    let events = mapped("redacted_thinking.sse");
    assert_eq!(&events[..2], &[reasoning("I check the path."), text("The path exists.")]);
    let Some(ProviderEvent::Done { provider_raw: Some(Value::String(raw)), .. }) = events.last()
    else {
        panic!("{events:?}");
    };
    assert!(
        raw.starts_with(r#"[{"type":"redacted_thinking","data":"EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpPkNRj2YfWXGmKDxH4mPnZ5sQ7vB5URj"},{"type":"thinking","#),
        "{raw}"
    );
}

#[test]
fn an_error_event_after_the_start_ends_the_stream() {
    let (events, error) = map_all(&fixture_events("error_mid_stream.sse"));
    assert_eq!(events, vec![text("Let me")]);
    match error {
        Some(ProviderError::Overloaded { status: None, message }) => {
            assert_eq!(message, "Overloaded")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn nothing_counts_after_the_stream_ended() {
    let mut mapper = EventMapper::new();
    let events = fixture_events("error_mid_stream.sse");
    let results: Vec<_> = events.iter().map(|event| mapper.map(event)).collect();
    assert!(results[3].is_err());
    assert_eq!(results[4].as_ref().unwrap(), &Vec::<ProviderEvent>::new());
}

#[rstest]
#[case::max_tokens("max_tokens.sse", StopReason::MaxTokens)]
#[case::refusal("refusal.sse", StopReason::ContentFilter)]
fn a_cut_answer_keeps_its_text(#[case] fixture: &str, #[case] stop_reason: StopReason) {
    let completion = fixture_completion(fixture);
    assert_eq!(completion.stop_reason, stop_reason);
    assert!(!completion.message.text().is_empty());
}

#[rstest]
#[case::end_turn("end_turn", false, StopReason::EndTurn)]
#[case::stop_sequence("stop_sequence", false, StopReason::EndTurn)]
#[case::tool_use("tool_use", true, StopReason::ToolUse)]
#[case::max_tokens("max_tokens", false, StopReason::MaxTokens)]
#[case::refusal("refusal", true, StopReason::ContentFilter)]
#[case::window_exceeded("model_context_window_exceeded", false, StopReason::MaxTokens)]
#[case::pause_turn("pause_turn", false, StopReason::EndTurn)]
#[case::unknown_with_a_call("a_new_reason", true, StopReason::ToolUse)]
#[case::unknown_without_a_call("a_new_reason", false, StopReason::EndTurn)]
fn each_stop_reason_maps_to_a_canonical_one(
    #[case] reason: &str,
    #[case] with_call: bool,
    #[case] expected: StopReason,
) {
    let mut events = vec![start()];
    events.extend(text_block(0, "Hi."));
    if with_call {
        events.push(json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "shell", "input": {}}}));
        events.push(json!({"type": "content_block_stop", "index": 1}));
    }
    events.extend(stop(reason));
    let (mapped, error) = map_all(&sse(&events));
    assert!(error.is_none(), "{error:?}");
    match mapped.last() {
        Some(ProviderEvent::Done { stop_reason, .. }) => assert_eq!(*stop_reason, expected),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_call_without_deltas_takes_the_input_of_its_start() {
    let mut events = vec![start()];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "shell", "input": {}}}));
    events.push(json!({"type": "content_block_stop", "index": 0}));
    events.extend(stop("tool_use"));
    let (mapped, error) = map_all(&sse(&events));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped[1],
        ProviderEvent::ToolCallEnd { call_id: "toolu_1".to_owned(), arguments: "{}".to_owned() }
    );
}

#[rstest]
#[case::cut_inside_the_input(Some(r#"{"command": "rm -rf /tmp/bu"#))]
#[case::not_an_object(Some("[1]"))]
#[case::never_stopped(None)]
fn a_call_that_is_not_whole_fails_the_stream(#[case] input: Option<&str>) {
    let mut events = vec![start()];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "shell", "input": {}}}));
    if let Some(input) = input {
        events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": input}}));
        events.push(json!({"type": "content_block_stop", "index": 0}));
    }
    events.extend(stop("max_tokens"));
    let (mapped, error) = map_all(&sse(&events));
    assert!(matches!(error, Some(ProviderError::InvalidStream { .. })), "{error:?}");
    assert!(!mapped.iter().any(|event| matches!(event, ProviderEvent::ToolCallEnd { .. })));
}

#[rstest]
#[case::too_long(
    json!({"type": "invalid_request_error", "message": "prompt is too long: 1000417 tokens > 1000000 maximum"}),
    "context_overflow"
)]
#[case::other_400(json!({"type": "invalid_request_error", "message": "messages: text content blocks must be non-empty"}), "api")]
#[case::authentication(json!({"type": "authentication_error", "message": "invalid x-api-key"}), "unauthorized")]
#[case::permission(json!({"type": "permission_error", "message": "no access"}), "api")]
#[case::billing(json!({"type": "billing_error", "message": "credit balance is too low"}), "api")]
#[case::not_found(json!({"type": "not_found_error", "message": "model: claude-x"}), "api")]
#[case::too_large(json!({"type": "request_too_large", "message": "Request exceeds the maximum size"}), "api")]
#[case::rate_limit(json!({"type": "rate_limit_error", "message": "slow down"}), "rate_limited")]
#[case::spend_limit(
    json!({"type": "rate_limit_error", "message": "spend limit", "details": {"error_code": "enforced_spend_limit_reached"}}),
    "api"
)]
#[case::api(json!({"type": "api_error", "message": "Internal server error"}), "api")]
#[case::timeout(json!({"type": "timeout_error", "message": "Timeout"}), "api")]
#[case::overloaded(json!({"type": "overloaded_error", "message": "Overloaded"}), "overloaded")]
#[case::unknown(json!({"type": "a_new_error", "message": "new"}), "api")]
fn an_error_event_has_the_class_of_its_type(#[case] error: Value, #[case] class: &str) {
    let events = sse(&[start(), json!({"type": "error", "error": error.clone()})]);
    let (_, failure) = map_all(&events);
    let message = error["message"].as_str().unwrap().to_owned();
    let kind = error["type"].as_str().unwrap().to_owned();
    match (failure, class) {
        (
            Some(ProviderError::ContextOverflow { status: None, code, message: text }),
            "context_overflow",
        ) => {
            assert_eq!((code.as_deref(), text), (Some(kind.as_str()), message));
        }
        (Some(ProviderError::Unauthorized { message: text }), "unauthorized") => {
            assert_eq!(text, Some(message));
        }
        (Some(ProviderError::RateLimited { retry_after: None }), "rate_limited") => {}
        (Some(ProviderError::Overloaded { status: None, message: text }), "overloaded") => {
            assert_eq!(text, message);
        }
        (Some(ProviderError::Api { status: None, code, message: text }), "api") => {
            let code = code.unwrap();
            assert!(code == kind || code == "enforced_spend_limit_reached", "{code}");
            assert_eq!(text, message);
        }
        (other, _) => panic!("{class}: {other:?}"),
    }
}

#[test]
fn an_error_event_without_an_error_object_still_fails() {
    let (_, error) = map_all(&sse(&[json!({"type": "error"})]));
    match error {
        Some(ProviderError::Api { code: None, message, .. }) => {
            assert_eq!(message, "the API sent an error event");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_events_blocks_and_deltas_are_passed_on() {
    let mut events = vec![start(), json!({"type": "message_progress", "share": 0.5})];
    events.push(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "zsh"}}}));
    events.push(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{}"}}));
    events.push(json!({"type": "content_block_stop", "index": 0}));
    events.extend(text_block(1, "Hi."));
    events.push(json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "late"}}));
    events.push(json!({"type": "ping"}));
    events.extend(stop("end_turn"));
    let (mapped, error) = map_all(&sse(&events));
    assert!(error.is_none(), "{error:?}");
    let raws: Vec<&Value> = mapped
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Raw(value) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(raws.len(), 2, "{mapped:?}");
    assert_eq!(raws[0]["type"], json!("message_progress"));
    assert_eq!(raws[1]["type"], json!("server_tool_use"));
    // The unknown block stays in the raw content as it came (`json!` sent its members
    // sorted); a delta after its block stopped is left out.
    assert_eq!(
        mapped.last(),
        Some(&done(
            StopReason::EndTurn,
            r#"[{"id":"srvtoolu_1","input":{"query":"zsh"},"name":"web_search","type":"server_tool_use"},{"type":"text","text":"Hi."}]"#,
        ))
    );
}

#[test]
fn an_answer_without_blocks_has_no_raw_content() {
    let mut events = vec![start()];
    events.extend(stop("end_turn"));
    let (mapped, _) = map_all(&sse(&events));
    assert_eq!(
        mapped.last(),
        Some(&ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None })
    );
}

#[test]
fn data_that_is_not_json_fails_the_stream() {
    let events = SseDecoder::new().push(b"event: message_start\ndata: {not json\n\n").unwrap();
    let (_, error) = map_all(&events);
    assert!(matches!(error, Some(ProviderError::Decode { .. })), "{error:?}");
}

#[test]
fn a_block_that_starts_twice_fails_the_stream() {
    let mut events = vec![start()];
    events.extend(text_block(0, "a"));
    events.extend(text_block(0, "b"));
    let (_, error) = map_all(&sse(&events));
    assert!(matches!(error, Some(ProviderError::InvalidStream { .. })), "{error:?}");
}
