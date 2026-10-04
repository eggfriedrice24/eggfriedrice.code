use std::str::FromStr;

use jiff::Timestamp;
use pretty_assertions::assert_eq;
use serde::Deserialize as _;
use serde_json::{Map, Value, json};

use crate::{
    CommandId, ConversationId, ErrorBody, ErrorCode, Event, EventEnvelope, Origin, PromptSend,
    PtyId, Scope, Seq, ShellContext, TurnId,
};

const TURN: &str = "01928c4e-7a3b-7c1d-8e2f-000000000001";
const CONVERSATION: &str = "01928c4e-7a3b-7c1d-8e2f-000000000002";
const PTY: &str = "01928c4e-7a3b-7c1d-8e2f-000000000003";

fn turn() -> TurnId {
    TurnId::from_str(TURN).unwrap()
}

#[test]
fn a_known_event_is_one_object_tagged_by_kind() {
    let event = Event::TurnStarted { turn_id: turn(), cwd: "/etc".into(), scope: Scope::Machine };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({ "kind": "turn_started", "turn_id": TURN, "cwd": "/etc", "scope": { "kind": "machine" } })
    );
}

/// A `prompt.send` whose last command holds a secret, decoded from the wire the way the
/// daemon receives it, with a copy of the command inside the context as an older client
/// might put it.
fn prompt_with_a_secret() -> PromptSend {
    serde_json::from_value(json!({
        "command_id": "01928c4e-7a3b-7c1d-8e2f-000000000004",
        "text": "why did that fail",
        "context": {
            "pwd": "/srv",
            "tty": "/dev/pts/3",
            "last_status": 1,
            "last_command": "export TOKEN=hunter2",
        },
        "last_command": "export TOKEN=hunter2",
    }))
    .unwrap()
}

#[test]
fn a_prompt_queued_event_never_contains_the_last_command() {
    let prompt = prompt_with_a_secret();
    assert_eq!(prompt.last_command.as_deref(), Some("export TOKEN=hunter2"));
    let event = Event::PromptQueued {
        turn_id: turn(),
        command_id: prompt.command_id,
        text: prompt.text.clone(),
        origin: Origin::Shell,
        context: prompt.context.clone(),
    };
    let envelope = EventEnvelope {
        seq: Seq::new(1),
        conversation_id: Some(ConversationId::from_str(CONVERSATION).unwrap()),
        at: Timestamp::UNIX_EPOCH,
        event: event.clone(),
    };
    for text in [
        serde_json::to_string(&event).unwrap(),
        serde_json::to_string(&envelope).unwrap(),
        format!("{event:?}"),
    ] {
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("last_command"), "{text}");
    }
    assert_eq!(
        serde_json::to_value(&event).unwrap()["context"],
        json!({ "pwd": "/srv", "tty": "/dev/pts/3", "last_status": 1 })
    );
}

#[test]
fn the_debug_output_of_a_prompt_leaves_out_the_last_command() {
    let prompt = prompt_with_a_secret();
    let text = format!("{prompt:?}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(text.contains("why did that fail"), "{text}");
}

#[test]
fn the_last_command_of_a_prompt_is_a_member_of_the_params_not_of_the_context() {
    let prompt = PromptSend {
        command_id: CommandId::from_str("01928c4e-7a3b-7c1d-8e2f-000000000004").unwrap(),
        conversation_id: None,
        new_conversation: false,
        text: "why".to_owned(),
        context: Some(ShellContext::new("/srv")),
        last_command: Some("make".to_owned()),
    };
    let value = serde_json::to_value(&prompt).unwrap();
    assert_eq!(value["last_command"], json!("make"));
    assert_eq!(value["context"], json!({ "pwd": "/srv" }));
    let back: PromptSend = serde_json::from_value(value).unwrap();
    assert_eq!(back, prompt);
}

#[test]
fn a_known_event_reads_back_as_itself() {
    let event = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Internal, "the provider stream broke"),
    };
    let back: Event = serde_json::from_value(serde_json::to_value(&event).unwrap()).unwrap();
    assert_eq!(back, event);
}

#[test]
fn an_unknown_kind_decodes_as_unknown_with_the_other_members() {
    let event: Event =
        serde_json::from_value(json!({ "kind": "device_enrolled", "label": "phone", "n": 1 }))
            .unwrap();
    let mut payload = Map::new();
    payload.insert("label".to_owned(), json!("phone"));
    payload.insert("n".to_owned(), json!(1));
    assert_eq!(event, Event::Unknown { kind: "device_enrolled".to_owned(), payload });
}

#[test]
fn an_unknown_event_encodes_back_to_the_same_bytes() {
    let text = r#"{"kind":"device_enrolled","label":"phone","nested":{"a":[1,2]}}"#;
    let event: Event = serde_json::from_str(text).unwrap();
    assert_eq!(serde_json::to_string(&event).unwrap(), text);
}

#[test]
fn the_literal_kind_unknown_is_not_a_variant_name() {
    let event: Event = serde_json::from_value(json!({ "kind": "unknown" })).unwrap();
    assert_eq!(event, Event::Unknown { kind: "unknown".to_owned(), payload: Map::new() });
}

#[test]
fn a_known_kind_with_a_malformed_body_is_an_error_not_unknown() {
    let missing_field = json!({ "kind": "turn_started", "cwd": "/etc" });
    assert!(serde_json::from_value::<Event>(missing_field).is_err());
    let wrong_type = json!({ "kind": "prompt_held", "turn_id": 7 });
    assert!(serde_json::from_value::<Event>(wrong_type).is_err());
}

#[test]
fn deserializing_through_the_trait_path_keeps_the_passthrough() {
    // A path call such as `Event::deserialize(..)` must reach the same code as
    // `serde_json::from_value`; an inherent `deserialize` would shadow the trait.
    let mut deserializer = serde_json::Deserializer::from_str(r#"{"kind":"device_enrolled"}"#);
    let event = <Event as serde::Deserialize>::deserialize(&mut deserializer).unwrap();
    let mut deserializer = serde_json::Deserializer::from_str(r#"{"kind":"device_enrolled"}"#);
    let by_path = Event::deserialize(&mut deserializer).unwrap();
    assert_eq!(by_path, event);
    assert_eq!(event.kind(), "device_enrolled");
}

#[test]
fn an_event_without_a_string_kind_is_an_error() {
    assert!(serde_json::from_value::<Event>(json!({ "turn_id": TURN })).is_err());
    assert!(serde_json::from_value::<Event>(json!({ "kind": 5 })).is_err());
    assert!(serde_json::from_value::<Event>(json!(["turn_started"])).is_err());
}

#[test]
fn a_hand_built_unknown_payload_never_writes_kind_twice() {
    let mut payload = Map::new();
    payload.insert("kind".to_owned(), json!("other"));
    payload.insert("x".to_owned(), json!(1));
    let event = Event::Unknown { kind: "future".to_owned(), payload };
    assert_eq!(serde_json::to_string(&event).unwrap(), r#"{"kind":"future","x":1}"#);
}

#[test]
fn kind_is_the_wire_tag() {
    let known = Event::TurnInterruptRequested { turn_id: turn(), origin: Origin::Phone };
    assert_eq!(known.kind(), "turn_interrupt_requested");
    let tag = serde_json::to_value(&known).unwrap()["kind"].clone();
    assert_eq!(tag, Value::from(known.kind()));

    let unknown = Event::Unknown { kind: "device_enrolled".to_owned(), payload: Map::new() };
    assert_eq!(unknown.kind(), "device_enrolled");
}

#[test]
fn turn_id_names_the_turn_of_turn_events_only() {
    assert_eq!(Event::TurnCancelled { turn_id: turn() }.turn_id(), Some(turn()));
    let shell = Event::ShellExited { pty_id: PtyId::from_str(PTY).unwrap(), exit_code: Some(0) };
    assert_eq!(shell.turn_id(), None);
    assert_eq!(Event::LoginCompleted { provider: "openai".to_owned() }.turn_id(), None);
}

#[test]
fn an_envelope_writes_its_time_as_rfc_3339_in_utc() {
    let envelope = EventEnvelope {
        seq: Seq::new(12),
        conversation_id: Some(ConversationId::from_str(CONVERSATION).unwrap()),
        at: Timestamp::from_str("2026-10-03T12:00:00.25Z").unwrap(),
        event: Event::TurnCancelled { turn_id: turn() },
    };
    assert_eq!(
        serde_json::to_value(&envelope).unwrap(),
        json!({
            "seq": 12,
            "conversation_id": CONVERSATION,
            "at": "2026-10-03T12:00:00.25Z",
            "event": { "kind": "turn_cancelled", "turn_id": TURN },
        })
    );
}

#[test]
fn an_envelope_reads_an_offset_time_and_writes_it_in_utc() {
    let envelope: EventEnvelope = serde_json::from_value(json!({
        "seq": 1,
        "at": "2026-10-03T14:00:00+02:00",
        "event": { "kind": "login_completed", "provider": "openai" },
    }))
    .unwrap();
    assert_eq!(envelope.conversation_id, None);
    assert_eq!(serde_json::to_value(&envelope).unwrap()["at"], json!("2026-10-03T12:00:00Z"));
}
