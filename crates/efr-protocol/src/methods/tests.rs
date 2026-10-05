use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    Base64Bytes, ConversationId, ConversationSubscribe, EffectiveSettings, InputRespond, Mode,
    OverriddenSettings, PageCursor, PromptSend, PromptSendResult, Seq, TurnSettings,
};

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";
const COMMAND: &str = "019a9b1c-3d00-7a10-8b20-000000000003";

#[test]
fn bytes_are_written_as_standard_base64_with_padding() {
    let bytes = Base64Bytes::new(b"ls\r".to_vec());
    assert_eq!(serde_json::to_value(&bytes).unwrap(), json!("bHMN"));
    assert_eq!(serde_json::to_value(Base64Bytes::new(b"a".to_vec())).unwrap(), json!("YQ=="));
}

#[test]
fn bytes_that_are_not_utf8_survive_a_round_trip() {
    let bytes = Base64Bytes::new(vec![0x1b, 0xff, 0x00, 0xe2, 0x82]);
    let back: Base64Bytes = serde_json::from_value(serde_json::to_value(&bytes).unwrap()).unwrap();
    assert_eq!(back.as_bytes(), [0x1b, 0xff, 0x00, 0xe2, 0x82]);
    assert_eq!(back.into_bytes(), vec![0x1b, 0xff, 0x00, 0xe2, 0x82]);
}

#[test]
fn empty_bytes_are_an_empty_string() {
    assert_eq!(serde_json::to_value(Base64Bytes::default()).unwrap(), json!(""));
}

#[test]
fn invalid_base64_is_rejected() {
    assert!(serde_json::from_value::<Base64Bytes>(json!("not base64!")).is_err());
    assert!(serde_json::from_value::<Base64Bytes>(json!([1, 2])).is_err());
}

#[test]
fn debug_shows_the_length_and_not_the_bytes() {
    let text = format!("{:?}", Base64Bytes::new(b"hunter2\r".to_vec()));
    assert_eq!(text, "Base64Bytes(<8 bytes>)");
}

#[test]
fn a_page_cursor_is_a_plain_string() {
    let cursor = PageCursor::new("c:41");
    assert_eq!(serde_json::to_value(&cursor).unwrap(), json!("c:41"));
    assert_eq!(cursor.as_str(), "c:41");
    let back: PageCursor = serde_json::from_value(json!("c:41")).unwrap();
    assert_eq!(back, cursor);
}

#[test]
fn a_subscribe_from_before_answers_input_still_parses_as_one_that_cannot_answer() {
    // The params of conversation.subscribe as clients sent them before answers_input.
    let old = r#"{"conversation_id":"019a9b1c-3d00-7a10-8b20-000000000001","after_seq":40}"#;
    let params: ConversationSubscribe = serde_json::from_str(old).unwrap();
    assert_eq!(params.conversation_id, CONVERSATION.parse::<ConversationId>().unwrap());
    assert_eq!(params.after_seq, Some(Seq::new(40)));
    assert!(!params.answers_input);
}

#[test]
fn answers_input_is_written_only_when_true() {
    let mut params = ConversationSubscribe {
        conversation_id: CONVERSATION.parse().unwrap(),
        after_seq: None,
        answers_input: false,
    };
    assert_eq!(serde_json::to_value(&params).unwrap(), json!({ "conversation_id": CONVERSATION }));
    params.answers_input = true;
    let value = serde_json::to_value(&params).unwrap();
    assert_eq!(value, json!({ "conversation_id": CONVERSATION, "answers_input": true }));
    let back: ConversationSubscribe = serde_json::from_value(value).unwrap();
    assert_eq!(back, params);
}

#[test]
fn answers_input_must_be_a_boolean() {
    let params = json!({ "conversation_id": CONVERSATION, "answers_input": "yes" });
    assert!(serde_json::from_value::<ConversationSubscribe>(params).is_err());
}

#[test]
fn the_longest_answer_is_frozen_with_the_wire_contract() {
    // Clients cap the line they read at this length, so a change is a protocol change.
    assert_eq!(InputRespond::MAX_TEXT_BYTES, 1024);
}

#[test]
fn a_prompt_from_before_turn_settings_parses_as_one_that_asks_for_none() {
    let old = json!({ "command_id": COMMAND, "text": "why" });
    let params: PromptSend = serde_json::from_value(old.clone()).unwrap();
    assert!(params.settings.is_empty());
    assert_eq!(serde_json::to_value(&params).unwrap(), old, "no settings are written");
}

#[test]
fn a_prompt_carries_the_settings_it_asks_for() {
    let mut params: PromptSend =
        serde_json::from_value(json!({ "command_id": COMMAND, "text": "why" })).unwrap();
    params.settings = TurnSettings { mode: Some(Mode::Manual), ..TurnSettings::default() };
    let value = serde_json::to_value(&params).unwrap();
    assert_eq!(value["settings"], json!({ "mode": "manual" }));
    let back: PromptSend = serde_json::from_value(value).unwrap();
    assert_eq!(back, params);
    assert!(format!("{params:?}").contains("Manual"), "{params:?}");
}

#[test]
fn a_prompt_with_an_unknown_mode_is_rejected() {
    let params = json!({ "command_id": COMMAND, "text": "why", "settings": { "mode": "yolo" } });
    assert!(serde_json::from_value::<PromptSend>(params).is_err());
}

#[test]
fn a_prompt_send_result_from_before_turn_settings_parses_without_settings() {
    let old = json!({ "conversation_id": CONVERSATION, "turn_id": TURN, "seq": 3, "queued": true });
    let result: PromptSendResult = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(result.settings, None);
    assert_eq!(serde_json::to_value(&result).unwrap(), old);
}

#[test]
fn a_prompt_send_result_reports_the_settings_of_its_turn() {
    let result = PromptSendResult {
        conversation_id: CONVERSATION.parse().unwrap(),
        turn_id: TURN.parse().unwrap(),
        seq: Seq::new(3),
        queued: false,
        settings: Some(EffectiveSettings {
            mode: Mode::Auto,
            model: "gpt-5.4".to_owned(),
            effort: None,
            overridden: OverriddenSettings { model: true, ..OverriddenSettings::default() },
        }),
    };
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(
        value["settings"],
        json!({ "mode": "auto", "model": "gpt-5.4", "overridden": { "model": true } })
    );
    let back: PromptSendResult = serde_json::from_value(value).unwrap();
    assert_eq!(back, result);
}
