use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{Base64Bytes, ConversationId, ConversationSubscribe, PageCursor, Seq};

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";

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
