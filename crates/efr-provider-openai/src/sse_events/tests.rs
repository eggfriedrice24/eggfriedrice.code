use std::time::Duration;

use efr_http::{SseDecoder, SseEvent};
use efr_provider::{
    CompletionBuilder, ContentBlock, ProviderError, ProviderEvent, StopReason, TokenUsage,
};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::{EventMapper, retry_hint};
use crate::testing::fixture_events;

/// The events of a stream made of `events`, each sent under its own `type` as the
/// event name, the way the Responses API frames them.
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

fn text(text: &str) -> ProviderEvent {
    ProviderEvent::TextDelta { text: text.to_owned() }
}

fn reasoning(text: &str) -> ProviderEvent {
    ProviderEvent::ReasoningDelta { text: text.to_owned() }
}

fn usage(input: u64, output: u64, cached: u64, reasoning: u64) -> ProviderEvent {
    ProviderEvent::Usage(TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
        reasoning_tokens: reasoning,
    })
}

/// The `item` of every `response.output_item.done` event of `events`, in order.
fn done_items(events: &[SseEvent]) -> Value {
    let items: Vec<Value> = events
        .iter()
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap())
        .filter(|data| data["type"] == "response.output_item.done")
        .map(|data| data["item"].clone())
        .collect();
    Value::Array(items)
}

#[test]
fn plain_text_streams_deltas_then_usage_then_done() {
    let events = fixture_events("plain_text.sse");
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped,
        vec![
            text("Your shell"),
            text(" is zsh"),
            text(" 5.9."),
            usage(1204, 9, 1024, 0),
            ProviderEvent::Done {
                stop_reason: StopReason::EndTurn,
                provider_raw: Some(done_items(&events)),
            },
        ]
    );
}

#[test]
fn plain_text_folds_into_one_assistant_message() {
    let (mapped, _) = map_all(&fixture_events("plain_text.sse"));
    let mut builder = CompletionBuilder::new();
    for event in &mapped {
        builder.push(event).unwrap();
    }
    let completion = builder.finish().unwrap();
    assert_eq!(completion.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(completion.stop_reason, StopReason::EndTurn);
    let raw = completion.message.provider_raw.unwrap();
    assert_eq!(raw[0]["id"], json!("msg_0a1b2c3d4e5f60718293a4b5c6d7e8f9"));
}

#[test]
fn a_tool_call_starts_grows_and_ends_with_the_whole_arguments() {
    let events = fixture_events("tool_call.sse");
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    let call_id = "call_Qm8sX2vR7nL4kP1a".to_owned();
    assert_eq!(
        mapped,
        vec![
            ProviderEvent::ToolCallStart { call_id: call_id.clone(), name: "shell".to_owned() },
            ProviderEvent::ToolCallDelta {
                call_id: call_id.clone(),
                arguments: "{\"command\":".to_owned(),
            },
            ProviderEvent::ToolCallDelta {
                call_id: call_id.clone(),
                arguments: "\"ls -la\"}".to_owned(),
            },
            ProviderEvent::ToolCallEnd {
                call_id: call_id.clone(),
                arguments: "{\"command\":\"ls -la\"}".to_owned(),
            },
            usage(1530, 46, 1280, 28),
            ProviderEvent::Done {
                stop_reason: StopReason::ToolUse,
                provider_raw: Some(done_items(&events)),
            },
        ]
    );
    let mut builder = CompletionBuilder::new();
    for event in &mapped {
        builder.push(event).unwrap();
    }
    let completion = builder.finish().unwrap();
    assert_eq!(
        completion.message.content,
        vec![ContentBlock::ToolCall {
            call_id,
            name: "shell".to_owned(),
            input: json!({"command": "ls -la"}),
        }]
    );
}

#[test]
fn reasoning_sections_are_separated_and_the_encrypted_item_is_kept() {
    let events = fixture_events("reasoning_round_trip.sse");
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    let items = done_items(&events);
    assert_eq!(
        items[0]["encrypted_content"],
        json!(
            "gAAAAABo4cF4reasoning_state_8Jk2Lm4Np6Qr8St0Uv2Wx4Yz6Ab8Cd0Ef2Gh4Ij6Kl8Mn0Op2Qr4St6Uv8Wx0Yz2Ab4Cd6Ef8Gh0"
        )
    );
    assert_eq!(
        mapped,
        vec![
            reasoning("**Checking the package manager**"),
            reasoning(" The machine runs Arch, so pacman."),
            reasoning("\n\n**Answering** No command is needed."),
            text("Use pacman: sudo pacman -Syu."),
            usage(980, 212, 0, 192),
            ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: Some(items) },
        ]
    );
}

#[test]
fn an_error_event_ends_the_stream_with_the_provider_error() {
    let (mapped, error) = map_all(&fixture_events("error_event.sse"));
    assert_eq!(mapped, vec![text("Checking")]);
    match error {
        Some(ProviderError::Api { status: None, code, message }) => {
            assert_eq!(code.as_deref(), Some("server_error"));
            assert_eq!(
                message,
                "The server had an error while processing your request. Sorry about that!"
            );
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[test]
fn an_error_event_may_nest_its_error() {
    let events = sse(&[json!({
        "type": "error",
        "error": {
            "type": "invalid_request_error",
            "code": "context_length_exceeded",
            "message": "Your input exceeds the context window of this model.",
        },
    })]);
    match map_all(&events).1 {
        Some(ProviderError::Api { code, message, .. }) => {
            assert_eq!(code.as_deref(), Some("context_length_exceeded"));
            assert_eq!(message, "Your input exceeds the context window of this model.");
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[test]
fn an_error_event_without_a_message_says_so() {
    let events = sse(&[json!({"type": "error"})]);
    match map_all(&events).1 {
        Some(ProviderError::Api { code: None, message, .. }) => {
            assert_eq!(message, "the provider sent an error event");
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[test]
fn a_failed_response_with_a_rate_limit_carries_the_wait() {
    let (mapped, error) = map_all(&fixture_events("failed_rate_limit.sse"));
    assert!(mapped.is_empty());
    match error {
        Some(ProviderError::RateLimited { retry_after }) => {
            assert_eq!(retry_after, Some(Duration::from_millis(11_054)));
        }
        other => panic!("not a rate limit: {other:?}"),
    }
}

#[test]
fn a_failed_response_reports_its_error() {
    let events = sse(&[json!({
        "type": "response.failed",
        "response": {"status": "failed", "error": {"code": "cyber_policy", "message": "Flagged."}},
    })]);
    match map_all(&events).1 {
        Some(ProviderError::Api { status: None, code, message }) => {
            assert_eq!(code.as_deref(), Some("cyber_policy"));
            assert_eq!(message, "Flagged.");
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[test]
fn the_output_limit_stops_the_answer_as_max_tokens() {
    let events = fixture_events("incomplete_max_tokens.sse");
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped,
        vec![
            text("The largest files are"),
            usage(600, 16, 0, 0),
            ProviderEvent::Done {
                stop_reason: StopReason::MaxTokens,
                provider_raw: Some(done_items(&events)),
            },
        ]
    );
}

#[test]
fn the_content_filter_stops_the_answer_as_filtered() {
    let events = sse(&[json!({
        "type": "response.incomplete",
        "response": {"incomplete_details": {"reason": "content_filter"}},
    })]);
    assert_eq!(
        map_all(&events).0,
        vec![ProviderEvent::Done { stop_reason: StopReason::ContentFilter, provider_raw: None }]
    );
}

#[test]
fn an_incomplete_response_for_another_reason_is_an_error() {
    let events = sse(&[json!({
        "type": "response.incomplete",
        "response": {"incomplete_details": {"reason": "interrupted"}},
    })]);
    match map_all(&events).1 {
        Some(ProviderError::Api { code, .. }) => assert_eq!(code.as_deref(), Some("interrupted")),
        other => panic!("not an API error: {other:?}"),
    }
}

#[test]
fn a_call_cut_off_by_the_limit_never_ends() {
    let events = sse(&[
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"id": "fc_1", "type": "function_call", "call_id": "call_1", "name": "shell", "arguments": ""},
        }),
        json!({"type": "response.function_call_arguments.delta", "item_id": "fc_1", "output_index": 0, "delta": "{\"command\":\"rm -rf /tmp/build"}),
        json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"}}}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(!mapped.iter().any(|event| matches!(event, ProviderEvent::ToolCallEnd { .. })));
    assert!(
        matches!(
            error,
            Some(ProviderError::InvalidStream {
                problem: "the response stopped inside a tool call"
            })
        ),
        "{error:?}"
    );
}

#[test]
fn a_completed_response_ends_a_call_whose_item_never_finished() {
    let events = sse(&[
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"id": "fc_1", "type": "function_call", "call_id": "call_1", "name": "shell", "arguments": ""},
        }),
        json!({"type": "response.function_call_arguments.delta", "item_id": "fc_1", "output_index": 0, "delta": "{\"command\":"}),
        json!({"type": "response.function_call_arguments.done", "item_id": "fc_1", "output_index": 0, "arguments": "{\"command\":\"pwd\"}"}),
        json!({"type": "response.completed", "response": {"output": []}}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped[2..],
        [
            ProviderEvent::ToolCallEnd {
                call_id: "call_1".to_owned(),
                arguments: "{\"command\":\"pwd\"}".to_owned(),
            },
            ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
        ]
    );
}

#[test]
fn items_sent_only_whole_still_reach_the_canonical_message() {
    // The shapes Codex's own tests send: finished items without deltas or indices.
    let events = sse(&[
        json!({"type": "response.created", "response": {"id": "resp_1"}}),
        json!({
            "type": "response.output_item.done",
            "item": {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "Thinking."}], "encrypted_content": "e30="},
        }),
        json!({
            "type": "response.output_item.done",
            "item": {"type": "message", "role": "assistant", "id": "msg_1", "content": [{"type": "output_text", "text": "Hello."}]},
        }),
        json!({
            "type": "response.output_item.done",
            "item": {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"command\":\"ls\"}"},
        }),
        json!({
            "type": "response.completed",
            "response": {"id": "resp_1", "usage": {"input_tokens": 0, "input_tokens_details": null, "output_tokens": 0, "output_tokens_details": null, "total_tokens": 0}},
        }),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        mapped,
        vec![
            reasoning("Thinking."),
            text("Hello."),
            ProviderEvent::ToolCallStart { call_id: "call_1".to_owned(), name: "shell".to_owned() },
            ProviderEvent::ToolCallEnd {
                call_id: "call_1".to_owned(),
                arguments: "{\"command\":\"ls\"}".to_owned(),
            },
            usage(0, 0, 0, 0),
            ProviderEvent::Done {
                stop_reason: StopReason::ToolUse,
                provider_raw: Some(done_items(&events)),
            },
        ]
    );
}

#[test]
fn the_response_output_stands_in_for_missing_finished_items() {
    let output = json!([{"type": "message", "id": "msg_1", "content": [{"type": "output_text", "text": "Hi"}]}]);
    let events = sse(&[
        json!({"type": "response.output_text.delta", "delta": "Hi"}),
        json!({"type": "response.completed", "response": {"output": output}}),
    ]);
    assert_eq!(
        map_all(&events).0,
        vec![
            text("Hi"),
            ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: Some(output) },
        ]
    );
}

#[test]
fn unknown_events_and_items_pass_on_as_raw() {
    let metadata = json!({"type": "response.metadata", "metadata": {"plan": "pro"}});
    let search = json!({"type": "web_search_call", "id": "ws_1", "status": "completed"});
    let events = sse(&[
        metadata.clone(),
        json!({"type": "response.output_item.done", "output_index": 0, "item": search}),
        json!({"type": "response.completed", "response": {}}),
    ]);
    assert_eq!(
        map_all(&events).0,
        vec![
            ProviderEvent::Raw(metadata),
            ProviderEvent::Raw(search.clone()),
            ProviderEvent::Done {
                stop_reason: StopReason::EndTurn,
                provider_raw: Some(json!([search])),
            },
        ]
    );
}

#[test]
fn known_events_without_a_canonical_form_are_consumed() {
    let events = sse(&[
        json!({"type": "response.created", "response": {"id": "resp_1"}}),
        json!({"type": "response.in_progress", "response": {"id": "resp_1"}}),
        json!({"type": "response.content_part.added", "item_id": "msg_1", "output_index": 0}),
        json!({"type": "response.output_text.done", "item_id": "msg_1", "output_index": 0, "text": "x"}),
        json!({"type": "response.reasoning_summary_part.done", "item_id": "rs_1", "output_index": 0}),
    ]);
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(mapped, Vec::new());
}

#[test]
fn data_that_is_not_json_is_a_decode_error() {
    let events = SseDecoder::new().push(b"event: response.created\ndata: {not json\n\n").unwrap();
    assert!(matches!(map_all(&events).1, Some(ProviderError::Decode { .. })));
}

#[test]
fn the_done_sentinel_and_events_after_done_are_ignored() {
    let mut events = fixture_events("plain_text.sse");
    events.extend(SseDecoder::new().push(b"data: [DONE]\n\n").unwrap());
    events.extend(sse(&[json!({"type": "response.output_text.delta", "delta": "late"})]));
    let (mapped, error) = map_all(&events);
    assert!(error.is_none(), "{error:?}");
    assert!(matches!(mapped.last(), Some(ProviderEvent::Done { .. })));
    assert!(!mapped.contains(&text("late")));
}

#[test]
fn arguments_for_a_call_that_never_started_are_left_out() {
    let events = sse(&[json!({
        "type": "response.function_call_arguments.delta",
        "item_id": "fc_9",
        "output_index": 3,
        "delta": "{}",
    })]);
    assert_eq!(map_all(&events).0, Vec::new());
}

#[test]
fn a_new_reasoning_item_after_another_starts_a_new_section() {
    let events = sse(&[
        json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "output_index": 0, "summary_index": 0, "delta": "First."}),
        json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_2", "output_index": 1, "summary_index": 0, "delta": "Second."}),
        json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 2, "delta": "Answer."}),
        json!({"type": "response.reasoning_text.delta", "item_id": "rs_3", "output_index": 3, "content_index": 0, "delta": "Raw."}),
    ]);
    assert_eq!(
        map_all(&events).0,
        vec![reasoning("First."), reasoning("\n\nSecond."), text("Answer."), reasoning("Raw.")]
    );
}

#[rstest]
#[case("Please try again in 11.054s.", Some(Duration::from_millis(11_054)))]
#[case("Please try again in 20ms.", Some(Duration::from_millis(20)))]
#[case("Please try again in 1m30s.", Some(Duration::from_secs(90)))]
#[case("Please try again in 2m.", Some(Duration::from_secs(120)))]
#[case("Try Again In 3s", Some(Duration::from_secs(3)))]
#[case("Please try again later.", None)]
#[case("try again in soon", None)]
#[case("Rate limit reached.", None)]
fn retry_hints(#[case] message: &str, #[case] wait: Option<Duration>) {
    assert_eq!(retry_hint(message), wait, "{message}");
}
