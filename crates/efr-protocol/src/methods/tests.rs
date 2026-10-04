use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{Base64Bytes, PageCursor};

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
