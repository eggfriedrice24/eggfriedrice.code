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
        search: None,
        drafts_coalesced: None,
        snapshot_live: None,
        approval_local_only: None,
        device_prove: None,
        conversations_watch: None,
        extra: BTreeMap::from([("zoom".to_owned(), json!(2)), ("pair".to_owned(), json!(true))]),
    };
    assert_eq!(serde_json::to_string(&caps).unwrap(), r#"{"admin":true,"pair":true,"zoom":2}"#);
}

#[test]
fn a_mix_of_known_and_unknown_keys_reads_back_as_itself() {
    let caps = Capabilities {
        admin: None,
        screen_snapshots: Some(true),
        search: Some(true),
        drafts_coalesced: Some(true),
        snapshot_live: None,
        approval_local_only: Some(false),
        device_prove: None,
        conversations_watch: None,
        extra: BTreeMap::from([("future".to_owned(), json!([1, 2]))]),
    };
    let back: Capabilities = serde_json::from_value(serde_json::to_value(&caps).unwrap()).unwrap();
    assert_eq!(back, caps);
}

#[test]
fn the_search_key_reads_into_its_field_and_its_absence_is_no_search() {
    let caps: Capabilities = serde_json::from_value(json!({ "search": true })).unwrap();
    assert_eq!(caps.search, Some(true));
    assert!(caps.extra.is_empty());
    assert!(!caps.is_empty());
    let old: Capabilities = serde_json::from_value(json!({ "admin": true })).unwrap();
    assert_eq!(old.search, None, "a daemon from before the search sends no key");
    let only = Capabilities { search: Some(false), ..Capabilities::default() };
    assert_eq!(serde_json::to_value(&only).unwrap(), json!({ "search": false }));
}

#[test]
fn the_remote_keys_read_into_their_fields_and_their_absence_is_an_older_daemon() {
    let caps: Capabilities = serde_json::from_value(json!({
        "drafts_coalesced": true,
        "snapshot_live": true,
        "approval_local_only": true,
        "device_prove": true,
    }))
    .unwrap();
    assert_eq!(caps.drafts_coalesced, Some(true));
    assert_eq!(caps.snapshot_live, Some(true));
    assert_eq!(caps.approval_local_only, Some(true));
    assert_eq!(caps.device_prove, Some(true));
    assert!(caps.extra.is_empty());
    assert!(!caps.is_empty());
    let old: Capabilities = serde_json::from_value(json!({ "search": true })).unwrap();
    assert_eq!(
        (old.drafts_coalesced, old.snapshot_live, old.approval_local_only, old.device_prove),
        (None, None, None, None)
    );
    for key in ["drafts_coalesced", "snapshot_live", "approval_local_only", "device_prove"] {
        let one: Capabilities = serde_json::from_value(json!({ key: false })).unwrap();
        assert!(!one.is_empty(), "{key}");
        assert_eq!(serde_json::to_value(&one).unwrap(), json!({ key: false }));
    }
}

#[test]
fn the_watch_key_reads_into_its_field_and_its_absence_is_an_older_daemon() {
    let caps: Capabilities =
        serde_json::from_value(json!({ "conversations_watch": true })).unwrap();
    assert_eq!(caps.conversations_watch, Some(true));
    assert!(caps.extra.is_empty());
    assert!(!caps.is_empty());
    let old: Capabilities = serde_json::from_value(json!({ "search": true })).unwrap();
    assert_eq!(old.conversations_watch, None, "a daemon from before the watch sends no key");
}
