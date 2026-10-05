use std::str::FromStr;

use jiff::Timestamp;
use pretty_assertions::assert_eq;
use serde::Deserialize as _;
use serde_json::{Map, Value, json};

use crate::{
    CallId, CommandId, ConversationId, EffectiveSettings, ErrorBody, ErrorCode, Event,
    EventEnvelope, InputWait, Mode, Origin, OverriddenSettings, PromptSend, PtyId, Scope, Seq,
    ShellContext, TurnId, TurnSettings,
};

const TURN: &str = "01928c4e-7a3b-7c1d-8e2f-000000000001";
const CONVERSATION: &str = "01928c4e-7a3b-7c1d-8e2f-000000000002";
const PTY: &str = "01928c4e-7a3b-7c1d-8e2f-000000000003";
const CALL: &str = "01928c4e-7a3b-7c1d-8e2f-000000000004";

fn turn() -> TurnId {
    TurnId::from_str(TURN).unwrap()
}

#[test]
fn a_known_event_is_one_object_tagged_by_kind() {
    let event = Event::TurnStarted {
        turn_id: turn(),
        cwd: "/etc".into(),
        scope: Scope::Machine,
        settings: None,
    };
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
        settings: prompt.settings.clone(),
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
        settings: TurnSettings::default(),
    };
    let value = serde_json::to_value(&prompt).unwrap();
    assert_eq!(value["last_command"], json!("make"));
    assert_eq!(value["context"], json!({ "pwd": "/srv" }));
    let back: PromptSend = serde_json::from_value(value).unwrap();
    assert_eq!(back, prompt);
}

#[test]
fn a_prompt_queued_event_from_before_turn_settings_still_parses_without_settings() {
    let old = json!({
        "kind": "prompt_queued",
        "turn_id": TURN,
        "command_id": "01928c4e-7a3b-7c1d-8e2f-000000000004",
        "text": "why",
        "origin": "shell",
    });
    let event: Event = serde_json::from_value(old.clone()).unwrap();
    let Event::PromptQueued { settings, .. } = &event else {
        panic!("{event:?}");
    };
    assert!(settings.is_empty());
    assert_eq!(serde_json::to_value(&event).unwrap(), old, "no settings are written");
}

#[test]
fn a_prompt_queued_event_keeps_the_settings_the_prompt_asked_for() {
    let settings = TurnSettings {
        mode: Some(Mode::Auto),
        model: Some("gpt-5.4".to_owned()),
        effort: Some("high".to_owned()),
    };
    let event = Event::PromptQueued {
        turn_id: turn(),
        command_id: CommandId::from_str("01928c4e-7a3b-7c1d-8e2f-000000000004").unwrap(),
        text: "why".to_owned(),
        origin: Origin::Shell,
        context: None,
        settings: settings.clone(),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["settings"], json!({ "mode": "auto", "model": "gpt-5.4", "effort": "high" }));
    let back: Event = serde_json::from_value(value).unwrap();
    assert_eq!(back, event);
}

#[test]
fn a_turn_started_event_from_before_turn_settings_parses_without_settings() {
    let old = json!({ "kind": "turn_started", "turn_id": TURN, "cwd": "/etc", "scope": { "kind": "machine" } });
    let event: Event = serde_json::from_value(old.clone()).unwrap();
    assert!(matches!(&event, Event::TurnStarted { settings: None, .. }), "{event:?}");
    assert_eq!(serde_json::to_value(&event).unwrap(), old);
}

#[test]
fn a_turn_started_event_records_the_settings_the_turn_runs_with() {
    let settings = EffectiveSettings {
        mode: Mode::Cautious,
        model: "gpt-5.5".to_owned(),
        effort: Some("medium".to_owned()),
        overridden: OverriddenSettings { effort: true, ..OverriddenSettings::default() },
    };
    let event = Event::TurnStarted {
        turn_id: turn(),
        cwd: "/etc".into(),
        scope: Scope::Machine,
        settings: Some(settings),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(
        value["settings"],
        json!({
            "mode": "cautious",
            "model": "gpt-5.5",
            "effort": "medium",
            "overridden": { "effort": true },
        })
    );
    let back: Event = serde_json::from_value(value).unwrap();
    assert_eq!(back, event);
}

#[test]
fn a_turn_started_event_with_malformed_settings_is_an_error_not_unknown() {
    let bad = json!({
        "kind": "turn_started",
        "turn_id": TURN,
        "cwd": "/etc",
        "scope": { "kind": "machine" },
        "settings": { "mode": "yolo", "model": "gpt-5.5" },
    });
    assert!(serde_json::from_value::<Event>(bad).is_err());
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

fn input_changed(input: InputWait) -> Event {
    Event::ToolCallInputChanged {
        turn_id: turn(),
        call_id: CallId::from_str(CALL).unwrap(),
        input,
        looks_secret: false,
    }
}

#[test]
fn an_input_change_names_the_call_and_what_it_waits_for() {
    let event = input_changed(InputWait::Hidden);
    assert_eq!(event.kind(), "tool_call_input_changed");
    assert_eq!(event.turn_id(), Some(turn()));
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({ "kind": "tool_call_input_changed", "turn_id": TURN, "call_id": CALL, "input": "hidden" })
    );
}

#[test]
fn every_input_wait_is_a_snake_case_string_and_reads_back_as_itself() {
    for (input, wire) in
        [(InputWait::None, "none"), (InputWait::Visible, "visible"), (InputWait::Hidden, "hidden")]
    {
        assert_eq!(serde_json::to_value(input).unwrap(), json!(wire));
        let event = input_changed(input);
        let back: Event = serde_json::from_value(serde_json::to_value(&event).unwrap()).unwrap();
        assert_eq!(back, event);
    }
}

#[test]
fn a_wait_that_looks_secret_says_so_and_an_old_one_does_not() {
    let event = Event::ToolCallInputChanged {
        turn_id: turn(),
        call_id: CallId::from_str(CALL).unwrap(),
        input: InputWait::Visible,
        looks_secret: true,
    };
    let wire = json!({
        "kind": "tool_call_input_changed",
        "turn_id": TURN,
        "call_id": CALL,
        "input": "visible",
        "looks_secret": true,
    });
    assert_eq!(serde_json::to_value(&event).unwrap(), wire);
    assert_eq!(serde_json::from_value::<Event>(wire).unwrap(), event);
    // An event from before the flag reads as not secret.
    let old = json!({ "kind": "tool_call_input_changed", "turn_id": TURN, "call_id": CALL, "input": "visible" });
    assert_eq!(serde_json::from_value::<Event>(old).unwrap(), input_changed(InputWait::Visible));
}

#[test]
fn no_input_wait_is_the_default() {
    assert_eq!(InputWait::default(), InputWait::None);
}

#[test]
fn an_input_change_with_an_unknown_wait_is_an_error_not_unknown() {
    let event = json!({ "kind": "tool_call_input_changed", "turn_id": TURN, "call_id": CALL, "input": "loud" });
    assert!(serde_json::from_value::<Event>(event).is_err());
}
