use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock as _;
use pretty_assertions::assert_eq;

use super::*;
use crate::testing::{self, TestClock, device, dump, on_writer, phone};

fn token(inside: &str) -> SecretText {
    SecretText::new(format!("ExponentPushToken[{inside}]"))
}

fn token_text(inside: &str) -> String {
    format!("ExponentPushToken[{inside}]")
}

/// The device id and the token of each target of `seen_since`.
async fn pushed(
    writer: &crate::WriterHandle,
    seen_since: Option<Timestamp>,
) -> Vec<(DeviceId, String)> {
    let targets = on_writer(writer, move |conn| targets(conn, seen_since)).await.unwrap();
    targets.into_iter().map(|t| (t.device_id, t.token.expose_secret().to_owned())).collect()
}

#[tokio::test]
async fn a_token_is_set_changed_and_removed() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    writer.device_insert(phone(2)).await.unwrap();

    assert!(writer.device_push_set(device(2), Some(token("b"))).await.unwrap());
    assert!(writer.device_push_set(device(1), Some(token("a"))).await.unwrap());
    let both = [(device(1), token_text("a")), (device(2), token_text("b"))];
    assert_eq!(pushed(&writer, None).await, both, "oldest pairing first");
    let named = on_writer(&writer, |conn| targets(conn, None)).await.unwrap();
    assert_eq!(named[0].name, "iPhone 1");

    assert!(writer.device_push_set(device(1), Some(token("c"))).await.unwrap());
    assert!(writer.device_push_set(device(2), None).await.unwrap());
    assert_eq!(pushed(&writer, None).await, [(device(1), token_text("c"))]);
    let with = on_writer(&writer, devices).await.unwrap();
    assert_eq!(with, HashSet::from([device(1)]));
}

#[tokio::test]
async fn an_unknown_or_revoked_device_gets_no_token_and_a_bad_token_fails() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    writer.device_revoke(device(1)).await.unwrap();

    assert!(!writer.device_push_set(device(1), Some(token("a"))).await.unwrap());
    assert!(!writer.device_push_set(device(2), Some(token("a"))).await.unwrap());
    writer.device_insert(phone(3)).await.unwrap();
    for bad in ["ExponentPushToken[]", "ExponentPushToken[a b]", "abc", "ExpoPushToken[a]"] {
        let set = writer.device_push_set(device(3), Some(SecretText::new(bad))).await;
        assert!(matches!(set, Err(StoreError::DeviceFieldInvalid { field: "token" })), "{bad}");
    }
    assert!(on_writer(&writer, |conn| Ok(dump(conn, TABLE))).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_revoke_deletes_the_token_in_its_own_transaction() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    writer.device_insert(phone(2)).await.unwrap();
    writer.device_push_set(device(1), Some(token("a"))).await.unwrap();
    writer.device_push_set(device(2), Some(token("b"))).await.unwrap();
    let mut revokes = writer.subscribe_revocations();

    writer.device_revoke(device(1)).await.unwrap();

    assert_eq!(revokes.try_recv().unwrap(), device(1));
    // The commit came first: a reader that gets the id finds no token.
    assert_eq!(pushed(&writer, None).await, [(device(2), token_text("b"))]);
}

#[tokio::test]
async fn a_dead_token_goes_only_while_the_row_still_holds_it() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    writer.device_push_set(device(1), Some(token("old"))).await.unwrap();
    writer.device_push_set(device(1), Some(token("new"))).await.unwrap();

    assert!(!writer.device_push_drop(device(1), token("old")).await.unwrap());
    assert_eq!(pushed(&writer, None).await, [(device(1), token_text("new"))]);
    assert!(writer.device_push_drop(device(1), token("new")).await.unwrap());
    assert_eq!(pushed(&writer, None).await, []);
}

#[tokio::test]
async fn seen_since_keeps_the_devices_that_proved_their_key_since() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    for n in 1..=3 {
        writer.device_insert(phone(n)).await.unwrap();
        writer.device_push_set(device(u64::from(n)), Some(token(&n.to_string()))).await.unwrap();
    }
    writer.device_touch(device(1)).await.unwrap();
    clock.advance(Duration::from_secs(60));
    let since = clock.now();
    writer.device_touch(device(2)).await.unwrap();

    let seen = pushed(&writer, Some(since)).await;

    assert_eq!(seen, [(device(2), token_text("2"))], "device 3 never connected");
    assert_eq!(pushed(&writer, None).await.len(), 3);
}

#[tokio::test]
async fn a_rebuild_of_the_projections_keeps_the_tokens() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    writer.device_push_set(device(1), Some(token("a"))).await.unwrap();

    writer.rebuild_projections().await.unwrap();

    assert_eq!(pushed(&writer, None).await, [(device(1), token_text("a"))]);
}

#[test]
fn the_debug_form_of_a_target_holds_no_token() {
    let target =
        PushTarget { device_id: device(1), name: "iPhone".to_owned(), token: token("secret") };
    assert!(!format!("{target:?}").contains("secret"));
}
