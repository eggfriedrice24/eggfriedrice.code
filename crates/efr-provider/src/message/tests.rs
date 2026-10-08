use efr_protocol::Base64Bytes;
use pretty_assertions::assert_eq;
use proptest::prelude::{Just, Strategy, any, prop, prop_assert_eq, prop_oneof, proptest};
use serde_json::{Value, json};

use super::{ContentBlock, Message, Role};

/// A Responses API reasoning item and function call, as `efr-provider-openai` stores
/// them: nested objects, arrays, an opaque blob, numbers and non-ASCII text.
fn responses_raw() -> Value {
    json!([
        {
            "type": "reasoning",
            "id": "rs_68af",
            "summary": [{"type": "summary_text", "text": "Listing the directory \u{2192} ls"}],
            "encrypted_content": "gAAAAABo...==",
        },
        {
            "type": "function_call",
            "id": "fc_68af",
            "call_id": "call_9xQ",
            "name": "shell",
            "arguments": "{\"command\":\"ls -la\"}",
            "status": "completed",
            "index": 0,
            "big": 18446744073709551615u64,
            "negative": -9223372036854775808i64,
            "nothing": null,
            "flag": false,
        },
    ])
}

#[test]
fn blocks_have_a_kind_tag() {
    let cases = [
        (ContentBlock::Text { text: "hi".to_owned() }, json!({"kind": "text", "text": "hi"})),
        (
            ContentBlock::ToolCall {
                call_id: "call_1".to_owned(),
                name: "shell".to_owned(),
                input: json!({"command": "ls"}),
                freeform: false,
            },
            json!({"kind": "tool_call", "call_id": "call_1", "name": "shell", "input": {"command": "ls"}}),
        ),
        (
            ContentBlock::ToolCall {
                call_id: "call_3".to_owned(),
                name: "apply_patch".to_owned(),
                input: json!("*** Begin Patch\n*** End Patch"),
                freeform: true,
            },
            json!({
                "kind": "tool_call",
                "call_id": "call_3",
                "name": "apply_patch",
                "input": "*** Begin Patch\n*** End Patch",
                "freeform": true,
            }),
        ),
        (
            ContentBlock::ToolResult {
                call_id: "call_1".to_owned(),
                output: "a\nb\n".to_owned(),
                is_error: false,
            },
            json!({"kind": "tool_result", "call_id": "call_1", "output": "a\nb\n"}),
        ),
        (
            ContentBlock::ToolResult {
                call_id: "call_2".to_owned(),
                output: "denied".to_owned(),
                is_error: true,
            },
            json!({"kind": "tool_result", "call_id": "call_2", "output": "denied", "is_error": true}),
        ),
        (
            ContentBlock::Reasoning { text: "think".to_owned() },
            json!({"kind": "reasoning", "text": "think"}),
        ),
        (
            ContentBlock::Image {
                media_type: "image/png".to_owned(),
                data: Base64Bytes::new(*b"\x89PNG"),
            },
            json!({"kind": "image", "media_type": "image/png", "data": "iVBORw=="}),
        ),
    ];
    for (block, wire) in cases {
        assert_eq!(serde_json::to_value(&block).unwrap(), wire);
        assert_eq!(serde_json::from_value::<ContentBlock>(wire).unwrap(), block);
    }
}

#[test]
fn roles_are_snake_case() {
    assert_eq!(serde_json::to_value(Role::User).unwrap(), json!("user"));
    assert_eq!(serde_json::to_value(Role::Assistant).unwrap(), json!("assistant"));
    assert!(serde_json::from_value::<Role>(json!("system")).is_err());
}

#[test]
fn a_message_without_raw_items_leaves_the_member_out() {
    let message = Message::user("what is in this directory?");
    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        json!({"role": "user", "content": [{"kind": "text", "text": "what is in this directory?"}]})
    );
}

#[test]
fn provider_raw_passes_through_a_round_trip_unchanged() {
    let message = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::Reasoning { text: "Listing the directory".to_owned() },
            ContentBlock::ToolCall {
                call_id: "call_9xQ".to_owned(),
                name: "shell".to_owned(),
                input: json!({"command": "ls -la"}),
                freeform: false,
            },
        ],
    )
    .with_provider_raw(responses_raw());

    let text = serde_json::to_string(&message).unwrap();
    let back: Message = serde_json::from_str(&text).unwrap();
    assert_eq!(back, message);
    assert_eq!(back.provider_raw, Some(responses_raw()));
    // A second trip is byte-identical, so storing and resending never drifts.
    assert_eq!(serde_json::to_string(&back).unwrap(), text);
}

#[test]
fn provider_raw_can_be_any_json_value() {
    for raw in [json!("opaque"), json!(42), json!(true), json!({}), json!([])] {
        let message = Message::assistant("ok").with_provider_raw(raw.clone());
        let back: Message =
            serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        assert_eq!(back.provider_raw, Some(raw));
    }
}

#[test]
fn a_null_raw_value_reads_back_as_absent() {
    let message = Message::assistant("ok").with_provider_raw(Value::Null);
    let back: Message = serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
    assert_eq!(back.provider_raw, None);
}

#[test]
fn decoding_ignores_unknown_fields_and_refuses_unknown_kinds() {
    let message: Message = serde_json::from_value(json!({
        "role": "assistant",
        "content": [{"kind": "text", "text": "hi", "annotations": []}],
        "model": "gpt-5",
    }))
    .unwrap();
    assert_eq!(message, Message::assistant("hi"));

    let unknown = json!({"role": "user", "content": [{"kind": "audio", "data": ""}]});
    assert!(serde_json::from_value::<Message>(unknown).is_err());
}

#[test]
fn text_joins_the_text_blocks_only() {
    let message = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::Reasoning { text: "hidden".to_owned() },
            ContentBlock::Text { text: "Checking.".to_owned() },
            ContentBlock::ToolCall {
                call_id: "c".to_owned(),
                name: "shell".to_owned(),
                input: json!({}),
                freeform: false,
            },
            ContentBlock::Text { text: "Done.".to_owned() },
        ],
    );
    assert_eq!(message.text(), "Checking.\n\nDone.");
    assert_eq!(Message::new(Role::User, Vec::new()).text(), "");
}

#[test]
fn image_bytes_stay_out_of_debug() {
    let block = ContentBlock::Image {
        media_type: "image/png".to_owned(),
        data: Base64Bytes::new(vec![7_u8; 1024]),
    };
    let debug = format!("{block:?}");
    assert!(debug.contains("<1024 bytes>"), "{debug}");
}

/// Any JSON value without floats (whose round trip serde_json makes best-effort
/// only) and without a top-level `null` (which reads back as absent).
fn raw_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<String>().prop_map(Value::String),
    ];
    let value = leaf.prop_recursive(4, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map(any::<String>(), inner, 0..6)
                .prop_map(|members| Value::Object(members.into_iter().collect())),
        ]
    });
    value.prop_filter("a top-level null reads back as absent", |value| !value.is_null())
}

proptest! {
    #[test]
    fn any_raw_value_survives_storage(raw in raw_value()) {
        let message = Message::assistant("ok").with_provider_raw(raw.clone());
        let back: Message = serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        prop_assert_eq!(back.provider_raw, Some(raw));
    }
}
