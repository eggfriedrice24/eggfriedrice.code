use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock as _;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use tokio::sync::broadcast::error::TryRecvError;

use super::*;
use crate::REVOCATION_CAPACITY;
use crate::testing::{self, TestClock, device, on_writer, phone};

fn key(first: u8, fill: u8) -> DevicePublicKey {
    let mut bytes = [fill; DevicePublicKey::LEN];
    bytes[0] = first;
    DevicePublicKey::from_x963(&bytes).unwrap()
}

fn peer() -> DevicePeer {
    DevicePeer {
        node_stable_id: "nAbC123CNTRL".to_owned(),
        user_id: 7_311_223_344,
        tailnet_stable_id: "TxYz987".to_owned(),
    }
}

fn new_device(name: &str) -> Result<NewDevice, StoreError> {
    NewDevice::new(device(1), name, key(0x04, 1), "SHA256:ab", peer(), &[ScopeName::Read])
}

fn invalid_field(result: Result<NewDevice, StoreError>) -> &'static str {
    match result {
        Err(StoreError::DeviceFieldInvalid { field }) => field,
        other => panic!("expected DeviceFieldInvalid, got {other:?}"),
    }
}

#[tokio::test]
async fn an_inserted_device_reads_back_from_get_and_list() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let new = NewDevice::new(
        device(1),
        "Ana's iPhone",
        key(0x04, 0xab),
        "SHA256:Zm9vYmFy",
        peer(),
        &[ScopeName::Approve, ScopeName::Read, ScopeName::Operate, ScopeName::Read],
    )
    .unwrap();

    let inserted = writer.device_insert(new).await.unwrap();

    let now = sql::truncate_to_micros(clock.now());
    let expected = Device {
        device_id: device(1),
        name: "Ana's iPhone".to_owned(),
        public_key: key(0x04, 0xab),
        key_fingerprint: "SHA256:Zm9vYmFy".to_owned(),
        node_stable_id: "nAbC123CNTRL".to_owned(),
        user_id: 7_311_223_344,
        tailnet_stable_id: "TxYz987".to_owned(),
        scopes: vec![ScopeName::Read, ScopeName::Operate, ScopeName::Approve],
        created_at: now,
        last_seen_at: None,
        revoked_at: None,
    };
    assert_eq!(inserted, expected);
    assert!(inserted.is_active());
    let read = on_writer(&writer, |conn| get(conn, device(1))).await.unwrap();
    assert_eq!(read, Some(expected.clone()));
    let all = on_writer(&writer, list).await.unwrap();
    assert_eq!(all, [expected]);
    let scopes = on_writer(&writer, |conn| {
        Ok(conn.query_row("SELECT scopes FROM devices", [], |row| row.get::<_, String>(0))?)
    })
    .await
    .unwrap();
    assert_eq!(scopes, r#"["read","operate","approve"]"#);
}

#[tokio::test]
async fn an_unknown_device_reads_as_none_and_the_list_starts_empty() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    assert_eq!(on_writer(&writer, |conn| get(conn, device(9))).await.unwrap(), None);
    assert_eq!(on_writer(&writer, list).await.unwrap(), []);
}

#[tokio::test]
async fn the_list_is_oldest_pairing_first_and_keeps_revoked_devices() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    for n in [3, 1, 2] {
        writer.device_insert(phone(n)).await.unwrap();
        clock.advance(Duration::from_secs(1));
    }
    writer.device_revoke(device(1)).await.unwrap();

    let all = on_writer(&writer, list).await.unwrap();

    let ids: Vec<_> = all.iter().map(|device| device.device_id).collect();
    assert_eq!(ids, [device(3), device(1), device(2)]);
    let active: Vec<_> = all.iter().map(Device::is_active).collect();
    assert_eq!(active, [true, false, true]);
}

#[tokio::test]
async fn a_second_insert_of_an_id_fails_and_changes_nothing() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let first = writer.device_insert(phone(1)).await.unwrap();
    let mut other_key = [9; DevicePublicKey::LEN];
    other_key[0] = 0x04;
    let again = NewDevice::new(
        device(1),
        "another name",
        DevicePublicKey::from_x963(&other_key).unwrap(),
        "SHA256:09",
        peer(),
        &[ScopeName::Read],
    )
    .unwrap();

    let refused = writer.device_insert(again).await;

    assert!(
        matches!(refused, Err(StoreError::DeviceExists { device_id }) if device_id == device(1)),
        "{refused:?}"
    );
    assert_eq!(on_writer(&writer, list).await.unwrap(), [first]);
}

#[tokio::test]
async fn a_key_names_one_active_device_and_is_free_again_after_its_revoke() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    let same_key = |n| {
        NewDevice::new(
            device(n),
            "the same phone",
            phone(1).public_key,
            "SHA256:01",
            peer(),
            &[ScopeName::Read],
        )
        .unwrap()
    };

    let refused = writer.device_insert(same_key(2)).await;
    assert!(
        matches!(refused, Err(StoreError::DeviceKeyInUse { device_id }) if device_id == device(1)),
        "{refused:?}"
    );

    writer.device_revoke(device(1)).await.unwrap();
    let again = writer.device_insert(same_key(2)).await.unwrap();
    assert!(again.is_active());
    assert_eq!(on_writer(&writer, list).await.unwrap().len(), 2);
}

#[tokio::test]
async fn the_index_refuses_a_second_active_row_of_a_key() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();

    let refused = on_writer(&writer, |conn| {
        Ok(conn.execute(
            "INSERT INTO devices SELECT ?1, name, public_key, key_fingerprint, node_stable_id, \
             user_id, tailnet_stable_id, scopes, created_at, NULL, NULL FROM devices",
            [device(2).to_string()],
        )?)
    })
    .await;

    assert!(refused.is_err(), "{refused:?}");
}

#[tokio::test]
async fn a_revoke_sets_its_time_once() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    writer.device_insert(phone(1)).await.unwrap();
    clock.advance(Duration::from_secs(60));
    let at = sql::truncate_to_micros(clock.now());

    let first = writer.device_revoke(device(1)).await.unwrap();
    clock.advance(Duration::from_secs(60));
    let second = writer.device_revoke(device(1)).await.unwrap();
    let unknown = writer.device_revoke(device(2)).await.unwrap();

    assert_eq!(first, RevokeOutcome::Revoked { at });
    assert_eq!(second, RevokeOutcome::AlreadyRevoked { at });
    assert_eq!(unknown, RevokeOutcome::Unknown);
    let read = on_writer(&writer, |conn| get(conn, device(1))).await.unwrap().unwrap();
    assert_eq!(read.revoked_at, Some(at));
    assert!(!read.is_active());
}

#[tokio::test]
async fn each_revoke_goes_once_to_every_subscriber_after_its_commit() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    writer.device_insert(phone(2)).await.unwrap();
    let mut first = writer.subscribe_revocations();
    let mut second = writer.subscribe_revocations();

    writer.device_revoke(device(2)).await.unwrap();
    writer.device_revoke(device(2)).await.unwrap();
    writer.device_revoke(device(3)).await.unwrap();

    for receiver in [&mut first, &mut second] {
        let id = receiver.try_recv().unwrap();
        assert_eq!(id, device(2));
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
    }
    // The commit came first: a reader that gets the id finds the row revoked.
    let read = on_writer(&writer, |conn| get(conn, device(2))).await.unwrap().unwrap();
    assert!(!read.is_active());
}

#[tokio::test]
async fn a_subscriber_that_lags_learns_it_and_reads_the_rows_again() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let mut receiver = writer.subscribe_revocations();
    let count = u8::try_from(REVOCATION_CAPACITY + 2).unwrap();
    for n in 1..=count {
        writer.device_insert(phone(n)).await.unwrap();
        writer.device_revoke(device(u64::from(n))).await.unwrap();
    }

    assert_eq!(receiver.try_recv(), Err(TryRecvError::Lagged(2)));
    let active = on_writer(&writer, list).await.unwrap().iter().filter(|d| d.is_active()).count();
    assert_eq!(active, 0);
}

#[tokio::test]
async fn a_revoke_of_no_subscriber_is_no_error() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    let outcome = writer.device_revoke(device(1)).await.unwrap();
    assert!(matches!(outcome, RevokeOutcome::Revoked { .. }), "{outcome:?}");
}

#[tokio::test]
async fn a_touch_moves_the_last_seen_time_of_an_active_device_only() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    writer.device_insert(phone(1)).await.unwrap();

    clock.advance(Duration::from_secs(5));
    assert!(writer.device_touch(device(1)).await.unwrap());
    let seen = sql::truncate_to_micros(clock.now());
    clock.advance(Duration::from_secs(5));
    assert!(writer.device_touch(device(1)).await.unwrap());
    let later = sql::truncate_to_micros(clock.now());
    let read = on_writer(&writer, |conn| get(conn, device(1))).await.unwrap().unwrap();
    assert!(seen < later);
    assert_eq!(read.last_seen_at, Some(later));

    writer.device_revoke(device(1)).await.unwrap();
    clock.advance(Duration::from_secs(5));
    assert!(!writer.device_touch(device(1)).await.unwrap());
    assert!(!writer.device_touch(device(2)).await.unwrap());
    let read = on_writer(&writer, |conn| get(conn, device(1))).await.unwrap().unwrap();
    assert_eq!(read.last_seen_at, Some(later));
}

#[tokio::test]
async fn a_rebuild_of_the_projections_keeps_the_devices() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    writer.device_insert(phone(2)).await.unwrap();
    writer.device_revoke(device(2)).await.unwrap();
    let before = on_writer(&writer, list).await.unwrap();

    writer.rebuild_projections().await.unwrap();

    assert_eq!(on_writer(&writer, list).await.unwrap(), before);
}

#[test]
fn a_key_must_be_65_bytes_that_start_with_4() {
    let mut bytes = [7u8; 66];
    bytes[0] = 0x04;
    assert!(DevicePublicKey::from_x963(&bytes[..65]).is_ok());
    for bad in [&bytes[..64], &bytes[..], &bytes[..33], &[][..]] {
        let refused = DevicePublicKey::from_x963(bad);
        assert!(
            matches!(refused, Err(StoreError::DeviceFieldInvalid { field: "public_key" })),
            "{} bytes: {refused:?}",
            bad.len()
        );
    }
    for first in [0x00, 0x02, 0x03, 0x06] {
        bytes[0] = first;
        assert!(DevicePublicKey::from_x963(&bytes[..65]).is_err(), "first byte {first}");
    }
    assert_eq!(key(0x04, 3).as_bytes()[1..], [3; 64]);
}

#[test]
fn the_debug_form_of_a_key_shows_only_its_start() {
    assert_eq!(format!("{:?}", key(0x04, 0xab)), "DevicePublicKey(04abababababababab..)");
}

#[test]
fn a_name_must_be_one_to_64_characters_that_a_terminal_shows_as_they_are() {
    assert!(new_device("Ana's iPhone").is_ok());
    assert!(new_device(&"é".repeat(DEVICE_NAME_MAX_CHARS)).is_ok());
    assert!(new_device("iPhone 17 Pro (work)").is_ok());
    for bad in [
        String::new(),
        " ".to_owned(),
        "\t\n".to_owned(),
        "a".repeat(DEVICE_NAME_MAX_CHARS + 1),
        "phone\u{1b}[2J".to_owned(),
        "phone\u{7}".to_owned(),
        "phone\u{0}".to_owned(),
        "phone\u{85}".to_owned(),
        "evil\u{202e}enohp".to_owned(),
        "phone\u{2066}x\u{2069}".to_owned(),
        "pho\u{200b}ne".to_owned(),
        "\u{feff}phone".to_owned(),
        "line\u{2028}break".to_owned(),
    ] {
        assert_eq!(invalid_field(new_device(&bad)), "name", "{bad:?}");
    }
}

#[test]
fn the_fingerprint_and_the_stable_ids_must_be_printable_ascii() {
    let with = |fingerprint: &str, node: &str, tailnet: &str| {
        let peer = DevicePeer {
            node_stable_id: node.to_owned(),
            user_id: 1,
            tailnet_stable_id: tailnet.to_owned(),
        };
        NewDevice::new(device(1), "phone", key(0x04, 1), fingerprint, peer, &[ScopeName::Read])
    };
    assert!(with("SHA256:ab", "n1", "t1").is_ok());
    let long = "x".repeat(DEVICE_TEXT_MAX_BYTES + 1);
    for bad in ["", "SHA256 ab", "SHA256:\u{1b}", "SHA256:é", long.as_str()] {
        assert_eq!(invalid_field(with(bad, "n1", "t1")), "key_fingerprint", "{bad:?}");
        assert_eq!(invalid_field(with("f", bad, "t1")), "node_stable_id", "{bad:?}");
        assert_eq!(invalid_field(with("f", "n1", bad)), "tailnet_stable_id", "{bad:?}");
    }
    assert!(with(&"x".repeat(DEVICE_TEXT_MAX_BYTES), "n1", "t1").is_ok());
}

#[test]
fn a_device_never_holds_the_admin_scope() {
    let with = |scopes: &[ScopeName]| {
        NewDevice::new(device(1), "phone", key(0x04, 1), "f", peer(), scopes)
    };
    assert_eq!(invalid_field(with(&[ScopeName::Read, ScopeName::Admin])), "scopes");
    assert_eq!(invalid_field(with(&ScopeName::ALL)), "scopes");
    assert!(with(&[]).is_ok());
    assert!(with(&[ScopeName::Terminal]).is_ok());
}

#[tokio::test]
async fn the_table_refuses_rows_of_the_wrong_shape() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let insert = |name: &'static str, key: Vec<u8>, scopes: &'static str| {
        let writer = writer.clone();
        async move {
            on_writer(&writer, move |conn| {
                Ok(conn.execute(
                    "INSERT INTO devices (device_id, name, public_key, key_fingerprint, \
                     node_stable_id, user_id, tailnet_stable_id, scopes, created_at) \
                     VALUES ('d', ?1, ?2, 'f', 'n', 1, 't', ?3, 0)",
                    rusqlite::params![name, key, scopes],
                )?)
            })
            .await
        }
    };
    let mut good = vec![7u8; 65];
    good[0] = 0x04;
    let mut compressed = vec![7u8; 33];
    compressed[0] = 0x02;
    let mut wrong_tag = good.clone();
    wrong_tag[0] = 0x05;

    assert!(insert("", good.clone(), "[]").await.is_err());
    assert!(insert("phone", compressed, "[]").await.is_err());
    assert!(insert("phone", wrong_tag, "[]").await.is_err());
    assert!(insert("phone", good.clone(), "{}").await.is_err());
    assert!(insert("phone", good.clone(), "not json").await.is_err());
    assert_eq!(insert("phone", good, "[]").await.unwrap(), 1);
}

#[tokio::test]
async fn a_scope_of_a_newer_build_fails_the_read_of_its_row() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    writer.device_insert(phone(1)).await.unwrap();
    on_writer(&writer, |conn| {
        Ok(conn.execute("UPDATE devices SET scopes = '[\"read\",\"teleport\"]'", [])?)
    })
    .await
    .unwrap();

    let read = on_writer(&writer, |conn| get(conn, device(1))).await;

    assert!(
        matches!(read, Err(StoreError::DecodeColumn { table: "devices", column: "scopes", .. })),
        "{read:?}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Each accepted name reads back as it was, and none of them holds a character that
    /// can change what the terminal shows around it.
    #[test]
    fn an_accepted_name_is_kept_as_it_is(name in "\\PC{0,70}|[\\x00-\\x7f\\u{200b}-\\u{202e}]{0,8}") {
        if let Ok(new) = new_device(&name) {
            prop_assert_eq!(&new.name, &name);
            prop_assert!(name.chars().count() <= DEVICE_NAME_MAX_CHARS);
            prop_assert!(!name.chars().any(|c| c.is_control() || is_invisible_format(c)));
        }
    }
}
