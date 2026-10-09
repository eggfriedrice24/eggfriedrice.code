use pretty_assertions::assert_eq;
use serde_json::json;

use super::{ProviderEvent, StopReason};
use crate::TokenUsage;

#[test]
fn events_have_a_kind_and_data() {
    let cases = [
        (
            ProviderEvent::TextDelta { text: "Listing".to_owned() },
            json!({"kind": "text_delta", "data": {"text": "Listing"}}),
        ),
        (
            ProviderEvent::ReasoningDelta { text: "Thinking".to_owned() },
            json!({"kind": "reasoning_delta", "data": {"text": "Thinking"}}),
        ),
        (
            ProviderEvent::ToolCallStart {
                call_id: "call_1".to_owned(),
                name: "shell".to_owned(),
                freeform: false,
            },
            json!({"kind": "tool_call_start", "data": {"call_id": "call_1", "name": "shell"}}),
        ),
        (
            ProviderEvent::ToolCallStart {
                call_id: "call_2".to_owned(),
                name: "apply_patch".to_owned(),
                freeform: true,
            },
            json!({"kind": "tool_call_start", "data": {"call_id": "call_2", "name": "apply_patch", "freeform": true}}),
        ),
        (
            ProviderEvent::ToolCallDelta {
                call_id: "call_1".to_owned(),
                arguments: "{\"comm".to_owned(),
            },
            json!({"kind": "tool_call_delta", "data": {"call_id": "call_1", "arguments": "{\"comm"}}),
        ),
        (
            ProviderEvent::ToolCallEnd {
                call_id: "call_1".to_owned(),
                arguments: "{\"command\":\"ls\"}".to_owned(),
            },
            json!({"kind": "tool_call_end", "data": {"call_id": "call_1", "arguments": "{\"command\":\"ls\"}"}}),
        ),
        (
            ProviderEvent::Usage(TokenUsage {
                input_tokens: 10,
                output_tokens: 2,
                cached_input_tokens: 0,
                reasoning_tokens: 0,
                cache_write_tokens: 8,
                cache_write_1h_tokens: 0,
            }),
            json!({"kind": "usage", "data": {
                "input_tokens": 10,
                "output_tokens": 2,
                "cached_input_tokens": 0,
                "reasoning_tokens": 0,
                "cache_write_tokens": 8,
                "cache_write_1h_tokens": 0,
            }}),
        ),
        (
            ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            json!({"kind": "done", "data": {"stop_reason": "end_turn"}}),
        ),
        (
            ProviderEvent::Done {
                stop_reason: StopReason::ToolUse,
                provider_raw: Some(json!([{"type": "function_call", "call_id": "call_1"}])),
            },
            json!({"kind": "done", "data": {
                "stop_reason": "tool_use",
                "provider_raw": [{"type": "function_call", "call_id": "call_1"}],
            }}),
        ),
        (
            ProviderEvent::Raw(json!({"type": "response.queued"})),
            json!({"kind": "raw", "data": {"type": "response.queued"}}),
        ),
        (ProviderEvent::Raw(json!("keep-alive")), json!({"kind": "raw", "data": "keep-alive"})),
    ];
    for (event, wire) in cases {
        assert_eq!(serde_json::to_value(&event).unwrap(), wire);
        assert_eq!(serde_json::from_value::<ProviderEvent>(wire).unwrap(), event);
    }
}

#[test]
fn stop_reasons_are_snake_case() {
    let cases = [
        (StopReason::EndTurn, "end_turn"),
        (StopReason::ToolUse, "tool_use"),
        (StopReason::MaxTokens, "max_tokens"),
        (StopReason::ContentFilter, "content_filter"),
    ];
    for (reason, wire) in cases {
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(wire));
        assert_eq!(serde_json::from_value::<StopReason>(json!(wire)).unwrap(), reason);
    }
}

#[test]
fn unknown_kinds_are_refused() {
    let wire = json!({"kind": "audio_delta", "data": {"bytes": ""}});
    assert!(serde_json::from_value::<ProviderEvent>(wire).is_err());
}
