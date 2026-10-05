use std::str::FromStr;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{CallId, ClientFrame, ConversationId, InputRespond, Method, RequestId, SecretText};

const CONVERSATION: &str = "01928c4e-7a3b-7c1d-8e2f-000000000002";
const CALL: &str = "01928c4e-7a3b-7c1d-8e2f-000000000004";

fn answer(text: &str, hidden: bool) -> InputRespond {
    InputRespond {
        conversation_id: ConversationId::from_str(CONVERSATION).unwrap(),
        call_id: CallId::from_str(CALL).unwrap(),
        text: SecretText::new(text),
        hidden,
        manual: false,
    }
}

#[test]
fn secret_text_is_a_plain_json_string() {
    let text = SecretText::new("hunter2");
    assert_eq!(serde_json::to_value(&text).unwrap(), json!("hunter2"));
    let back: SecretText = serde_json::from_value(json!("hunter2")).unwrap();
    assert_eq!(back, text);
    assert_eq!(back.expose_secret(), "hunter2");
}

#[test]
fn secret_text_keeps_any_string_in_a_round_trip() {
    for raw in ["", "y", "pass word", "p\u{e4}ss\u{1f511}", "a\"b\\c"] {
        let text = SecretText::new(raw);
        let back: SecretText =
            serde_json::from_str(&serde_json::to_string(&text).unwrap()).unwrap();
        assert_eq!(back.expose_secret(), raw);
    }
}

#[test]
fn secret_text_rejects_what_is_not_a_string() {
    assert!(serde_json::from_value::<SecretText>(json!(7)).is_err());
    assert!(serde_json::from_value::<SecretText>(json!(["hunter2"])).is_err());
    assert!(serde_json::from_value::<SecretText>(json!(null)).is_err());
}

#[test]
fn the_debug_output_of_secret_text_is_a_placeholder() {
    assert_eq!(format!("{:?}", SecretText::new("hunter2")), "SecretText(<redacted>)");
    assert_eq!(format!("{:#?}", SecretText::new("hunter2")), "SecretText(<redacted>)");
}

#[test]
fn the_debug_output_of_an_answer_never_holds_its_text() {
    let params = answer("hunter2", true);
    let method = Method::InputRespond(params.clone());
    let frame = ClientFrame::Request { id: RequestId::new(3), method: method.clone() };
    for text in [
        format!("{params:?}"),
        format!("{params:#?}"),
        format!("{method:?}"),
        format!("{method:#?}"),
        format!("{frame:?}"),
    ] {
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
    }
}

#[test]
fn an_answer_is_written_with_its_text_in_clear_on_the_wire() {
    // The wire must carry the text to the daemon; only logs and Debug hide it.
    assert_eq!(
        serde_json::to_value(Method::InputRespond(answer("y", false))).unwrap(),
        json!({
            "method": "input.respond",
            "params": {
                "conversation_id": CONVERSATION,
                "call_id": CALL,
                "text": "y",
                "hidden": false,
            },
        })
    );
}

#[test]
fn an_answer_reads_back_as_itself() {
    for params in [answer("hunter2", true), answer("y", false)] {
        let method = Method::InputRespond(params);
        let back: Method = serde_json::from_value(serde_json::to_value(&method).unwrap()).unwrap();
        assert_eq!(back, method);
    }
}

#[test]
fn a_manual_answer_says_so_and_an_answer_without_the_flag_is_not_manual() {
    let manual = InputRespond { manual: true, ..answer("y", false) };
    let wire = serde_json::to_value(Method::InputRespond(manual.clone())).unwrap();
    assert_eq!(wire["params"]["manual"], json!(true));
    let back: Method = serde_json::from_value(wire).unwrap();
    assert_eq!(back, Method::InputRespond(manual));

    let params =
        json!({ "conversation_id": CONVERSATION, "call_id": CALL, "text": "y", "hidden": false });
    assert!(!serde_json::from_value::<InputRespond>(params).unwrap().manual);
}

#[test]
fn an_answer_without_hidden_is_rejected() {
    // `hidden` decides whether the daemon requires echo off, so it is never implied.
    let params = json!({ "conversation_id": CONVERSATION, "call_id": CALL, "text": "y" });
    let err = serde_json::from_value::<InputRespond>(params).unwrap_err();
    assert!(err.to_string().contains("hidden"), "{err}");
}
