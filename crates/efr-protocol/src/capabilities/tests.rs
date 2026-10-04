use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::Capabilities;

#[test]
fn no_capabilities_is_an_empty_object() {
    assert_eq!(serde_json::to_value(Capabilities::default()).unwrap(), json!({}));
    assert!(Capabilities::default().is_empty());
}

#[test]
fn known_keys_read_into_their_fields() {
    let caps: Capabilities =
        serde_json::from_value(json!({ "admin": true, "screen_snapshots": false })).unwrap();
    assert_eq!(caps.admin, Some(true));
    assert_eq!(caps.screen_snapshots, Some(false));
    assert!(caps.extra.is_empty());
}

#[test]
fn unknown_keys_are_kept_as_extras() {
    let caps: Capabilities =
        serde_json::from_value(json!({ "admin": true, "pair": { "qr": true }, "zoom": 2 }))
            .unwrap();
    assert_eq!(
        caps.extra,
        BTreeMap::from([("pair".to_owned(), json!({ "qr": true })), ("zoom".to_owned(), json!(2))])
    );
    assert!(!caps.is_empty());
}

#[test]
fn extras_are_written_after_the_known_keys_in_name_order() {
    let caps = Capabilities {
        admin: Some(true),
        screen_snapshots: None,
        extra: BTreeMap::from([("zoom".to_owned(), json!(2)), ("pair".to_owned(), json!(true))]),
    };
    assert_eq!(serde_json::to_string(&caps).unwrap(), r#"{"admin":true,"pair":true,"zoom":2}"#);
}

#[test]
fn a_mix_of_known_and_unknown_keys_reads_back_as_itself() {
    let caps = Capabilities {
        admin: None,
        screen_snapshots: Some(true),
        extra: BTreeMap::from([("future".to_owned(), json!([1, 2]))]),
    };
    let back: Capabilities = serde_json::from_value(serde_json::to_value(&caps).unwrap()).unwrap();
    assert_eq!(back, caps);
}
