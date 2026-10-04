use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Bindings, is_placeholder};

const A: &str = "019a9b1c-3d00-7a10-8b20-00000000000a";
const B: &str = "019a9b1c-3d00-7a10-8b20-00000000000b";
const C: &str = "019a9b1c-3d00-7a10-8b20-00000000000c";

#[test]
fn ids_are_numbered_per_kind_in_the_order_they_are_seen() {
    let mut bindings = Bindings::new();

    let result = bindings.normalize(&json!({ "conversation_id": A, "turn_id": B, "seq": 2 }));
    let event = bindings.normalize(&json!({
        "kind": "tool_call_started",
        "turn_id": B,
        "call_id": C,
        "input": { "command": "ls", "turn_id": A },
    }));

    assert_eq!(
        result,
        json!({ "conversation_id": "<conversation:1>", "turn_id": "<turn:1>", "seq": 2 })
    );
    assert_eq!(
        event,
        json!({
            "kind": "tool_call_started",
            "turn_id": "<turn:1>",
            "call_id": "<call:1>",
            "input": { "command": "ls", "turn_id": "<conversation:1>" },
        }),
        "an id keeps its first placeholder wherever it shows up again"
    );
    assert_eq!(bindings.get("<call:1>"), Some(C));
    assert_eq!(bindings.placeholder(A), Some("<conversation:1>"));
    assert_eq!(bindings.get("<call:2>"), None);
}

#[test]
fn only_id_members_are_replaced() {
    let mut bindings = Bindings::new();
    let value = json!({ "command_id": A, "text": B, "items": [{ "pty_id": C }], "turn_id": 5 });

    let normalized = bindings.normalize(&value);

    assert_eq!(
        normalized,
        json!({ "command_id": A, "text": B, "items": [{ "pty_id": "<pty:1>" }], "turn_id": 5 })
    );
}

#[test]
fn a_placeholder_in_a_record_is_left_as_it_is() {
    let mut bindings = Bindings::new();

    assert_eq!(
        bindings.normalize(&json!({ "turn_id": "<turn:7>" })),
        json!({ "turn_id": "<turn:7>" })
    );
    assert_eq!(bindings.get("<turn:1>"), None);
}

#[test]
fn substitution_puts_the_ids_back_inside_any_string() {
    let mut bindings = Bindings::new();
    bindings.normalize(&json!({ "conversation_id": A, "call_id": B }));

    let frame = json!({
        "id": 2,
        "params": { "conversation_id": "<conversation:1>", "call_id": "<call:1>", "note": "x <call:1> <b> <CWD>" },
    });

    assert_eq!(
        bindings.substitute(&frame).unwrap(),
        json!({
            "id": 2,
            "params": { "conversation_id": A, "call_id": B, "note": format!("x {B} <b> <CWD>") },
        })
    );
    assert_eq!(bindings.substitute(&json!(["<turn:1>"])), Err("<turn:1>".to_owned()));
}

#[test]
fn a_placeholder_has_a_known_kind_and_a_number() {
    for good in ["<turn:1>", "<conversation:12>", "<call:3>", "<pty:9>"] {
        assert!(is_placeholder(good), "{good}");
    }
    for bad in ["<turn:>", "<turn:x>", "<CWD>", "<command:1>", "turn:1", "<turn:1", "<turn1>"] {
        assert!(!is_placeholder(bad), "{bad}");
    }
}
