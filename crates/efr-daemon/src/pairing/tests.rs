use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{DeviceId, Origin, PairEnd, ScopeName, SecretText};
use efr_stdx::time::Clock as _;
use efr_store::devices::DevicePublicKey;
use efr_test_support::{TestClock, TestRng, TestStore};
use efr_transport::{ConnId, ConnectionContext, PeerCred, RemotePeer};
use pretty_assertions::assert_eq;

use super::{
    Answer, CONFIRM_WINDOW, DEVICE_SCOPES, Decision, EnrollPeer, MAX_FAILED_ENROLLS, MAX_OPEN,
    PAIRING_TTL, Pairings, PendingFacts, TOKEN_BYTES, Update, enroll_peer, key_fingerprint, same,
    token_hash,
};
use crate::DaemonError;
use crate::error::PairingProblem;
use crate::methods::granted;
use crate::sandbox::peers::PeerSide;

fn pairings(clock: &TestClock) -> Pairings {
    Pairings::new(clock.shared(), Arc::new(TestRng::new(11)))
}

/// A key of the right shape: `04`, then the bytes 1 to 64.
fn key() -> DevicePublicKey {
    let mut bytes = [0x04; DevicePublicKey::LEN];
    for (at, byte) in bytes.iter_mut().enumerate().skip(1) {
        *byte = u8::try_from(at).unwrap();
    }
    DevicePublicKey::from_x963(&bytes).unwrap()
}

fn peer() -> EnrollPeer {
    EnrollPeer {
        node_stable_id: "nPHONE1CNTRL".to_owned(),
        node_name: "annas-iphone.tail1234.ts.net.".to_owned(),
        user_id: 42,
        user_login: "anna@example.com".to_owned(),
        tailnet_stable_id: "TtAiLnEt1".to_owned(),
        os: Some("iOS".to_owned()),
    }
}

fn wrong() -> SecretText {
    SecretText::new("AAAAAAAAAAAAAAAAAAAAAAAA")
}

#[test]
fn a_token_has_144_random_bits_and_efrd_keeps_only_its_hash() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);

    let opened = pairings.open(None).unwrap();

    assert_eq!(TOKEN_BYTES * 8, 144);
    assert_eq!(opened.token.expose_secret().len(), 24, "18 bytes are 24 base64url characters");
    assert!(
        opened
            .token
            .expose_secret()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    let table = super::lock(&pairings.table);
    let entry = table.open.get(&opened.pairing_id).unwrap();
    assert_eq!(entry.hash, token_hash(&opened.token));
    assert_eq!(entry.expires_at, super::after(clock.now(), PAIRING_TTL));
    drop(table);
    let shown = format!("{pairings:?} {opened:?}");
    assert!(!shown.contains(opened.token.expose_secret()), "{shown}");
}

#[test]
fn two_pairings_get_two_tokens() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);

    let first = pairings.open(None).unwrap();
    let second = pairings.open(None).unwrap();

    assert_ne!(first.token.expose_secret(), second.token.expose_secret());
    assert_ne!(first.pairing_id, second.pairing_id);
}

#[tokio::test]
async fn a_yes_writes_the_device_with_the_whois_facts_and_tells_both_sides() {
    let clock = TestClock::new();
    let store = TestStore::open(clock.shared()).await.unwrap();
    let pairings = pairings(&clock);
    let mut opened = pairings.open(None).unwrap();

    let id = pairings.check(&opened.token).unwrap();
    let wait = pairings.enroll(id, &opened.token, "Anna's iPhone", key(), peer()).unwrap();

    let facts = PendingFacts {
        device_name: "Anna's iPhone".to_owned(),
        node_name: "annas-iphone.tail1234.ts.net".to_owned(),
        user_login: "anna@example.com".to_owned(),
        key_fingerprint: "SHA256:0ed3a6ab957ff6f5".to_owned(),
        os: Some("iOS".to_owned()),
    };
    assert_eq!(opened.next().await, Some(Update::Pending(facts)));
    let Decision::Accepted(accepted) = pairings.answer(id, true).unwrap() else {
        panic!("a yes accepts");
    };
    let device_id = accepted.complete(store.writer().clone()).await.unwrap();

    let scopes = vec![ScopeName::Read, ScopeName::Operate, ScopeName::Approve];
    assert_eq!(wait.outcome().await, Answer::Paired { device_id, scopes });
    assert_eq!(opened.next().await, Some(Update::Paired { device_id }));
    let device =
        store.readers().with(move |conn| efr_store::devices::get(conn, device_id)).await.unwrap();
    let device = device.unwrap();
    assert_eq!(device.name, "Anna's iPhone");
    assert_eq!(device.public_key, key());
    assert_eq!(device.key_fingerprint, "SHA256:0ed3a6ab957ff6f5");
    assert_eq!(
        (device.node_stable_id.as_str(), device.user_id, device.tailnet_stable_id.as_str()),
        ("nPHONE1CNTRL", 42, "TtAiLnEt1")
    );
    assert!(device.is_active());
    assert_eq!(pairings.open_count(), 0);
}

#[tokio::test]
async fn the_name_of_pair_start_wins_over_the_phones() {
    let clock = TestClock::new();
    let store = TestStore::open(clock.shared()).await.unwrap();
    let pairings = pairings(&clock);
    let opened = pairings.open(Some("work phone".to_owned())).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let _wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();

    let Decision::Accepted(accepted) = pairings.answer(id, true).unwrap() else {
        panic!("a yes accepts");
    };
    let device_id = accepted.complete(store.writer().clone()).await.unwrap();

    let device =
        store.readers().with(move |conn| efr_store::devices::get(conn, device_id)).await.unwrap();
    assert_eq!(device.unwrap().name, "work phone");
}

#[tokio::test]
async fn a_no_ends_the_pairing_as_refused_on_both_sides() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let mut opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();
    assert!(matches!(opened.next().await, Some(Update::Pending(_))));

    assert!(matches!(pairings.answer(id, false).unwrap(), Decision::Refused));

    assert_eq!(wait.outcome().await, Answer::Ended(PairEnd::Refused));
    assert_eq!(opened.next().await, Some(Update::Ended(PairEnd::Refused)));
    assert_eq!(
        pairings.answer(id, true).unwrap_err(),
        PairingProblem::NotPending { pairing_id: id }
    );
}

#[test]
fn a_token_is_single_use() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let _wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();

    assert_eq!(pairings.check(&opened.token), Err(PairingProblem::BadToken));
    assert_eq!(
        pairings.enroll(id, &opened.token, "iPhone 2", key(), peer()).unwrap_err(),
        PairingProblem::BadToken
    );
    assert_eq!(super::lock(&pairings.table).failed, 0, "the right token is no failed attempt");
}

#[test]
fn an_answer_before_any_phone_is_not_found() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let opened = pairings.open(None).unwrap();

    let refused = pairings.answer(opened.pairing_id, true).unwrap_err();

    assert_eq!(refused, PairingProblem::NotPending { pairing_id: opened.pairing_id });
    assert_eq!(pairings.open_count(), 1, "the pairing still waits for its phone");
}

#[test]
fn a_token_expires_after_five_minutes() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let opened = pairings.open(None).unwrap();

    clock.advance(PAIRING_TTL - Duration::from_secs(1));
    assert!(pairings.check(&opened.token).is_ok());
    assert!(!opened.expire(clock.now()));
    clock.advance(Duration::from_secs(1));

    assert_eq!(pairings.check(&opened.token), Err(PairingProblem::BadToken));
    assert!(opened.expire(clock.now()));
    assert_eq!(pairings.open_count(), 0);
}

#[tokio::test]
async fn the_terminal_gets_the_confirm_window_after_a_late_enroll() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let opened = pairings.open(None).unwrap();
    clock.advance(PAIRING_TTL - Duration::from_secs(10));
    let id = pairings.check(&opened.token).unwrap();
    let wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();

    assert_eq!(opened.deadline(), Some(super::after(clock.now(), CONFIRM_WINDOW)));
    clock.advance(CONFIRM_WINDOW - Duration::from_secs(1));
    assert!(!opened.expire(clock.now()), "the expiry of the token alone does not end it");
    clock.advance(Duration::from_secs(1));
    assert!(opened.expire(clock.now()));

    assert_eq!(wait.outcome().await, Answer::Ended(PairEnd::Expired));
}

#[tokio::test]
async fn too_many_wrong_tokens_end_the_pairings_that_wait_for_a_phone() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let mut waiting = pairings.open(None).unwrap();
    let mut answered = pairings.open(None).unwrap();
    let id = pairings.check(&answered.token).unwrap();
    let _wait = pairings.enroll(id, &answered.token, "iPhone", key(), peer()).unwrap();

    for _ in 0..MAX_FAILED_ENROLLS {
        assert_eq!(pairings.check(&wrong()), Err(PairingProblem::BadToken));
    }
    assert_eq!(pairings.open_count(), 2, "the cap is not reached yet");
    assert_eq!(pairings.check(&wrong()), Err(PairingProblem::BadToken));

    assert_eq!(waiting.next().await, Some(Update::Ended(PairEnd::Attempts)));
    assert_eq!(pairings.check(&waiting.token), Err(PairingProblem::BadToken));
    assert!(matches!(answered.next().await, Some(Update::Pending(_))));
    assert_eq!(pairings.open_count(), 1, "the pairing that has its phone stays");
}

#[test]
fn wrong_tokens_count_only_while_a_pairing_is_open_and_a_new_pairing_starts_from_zero() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    for _ in 0..10 {
        assert_eq!(pairings.check(&wrong()), Err(PairingProblem::BadToken));
    }
    assert_eq!(super::lock(&pairings.table).failed, 0);

    let first = pairings.open(None).unwrap();
    for _ in 0..3 {
        let _ = pairings.check(&wrong());
    }
    drop(first);
    let second = pairings.open(None).unwrap();

    assert_eq!(super::lock(&pairings.table).failed, 0);
    assert!(pairings.check(&second.token).is_ok());
}

#[tokio::test]
async fn a_pairing_that_waits_for_the_terminal_does_not_keep_the_count_for_the_next() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let mut waiting = pairings.open(None).unwrap();
    let answered = pairings.open(None).unwrap();
    let id = pairings.check(&answered.token).unwrap();
    let _wait = pairings.enroll(id, &answered.token, "iPhone", key(), peer()).unwrap();
    for _ in 0..=MAX_FAILED_ENROLLS {
        let _ = pairings.check(&wrong());
    }
    assert_eq!(waiting.next().await, Some(Update::Ended(PairEnd::Attempts)));
    assert_eq!(pairings.open_count(), 1, "only the pairing that has its phone is open");

    // Wrong tokens while no pairing waits for a phone protect nothing.
    let _ = pairings.check(&wrong());
    let next = pairings.open(None).unwrap();
    assert_eq!(super::lock(&pairings.table).failed, 0, "a new pairing starts from zero");
    assert_eq!(pairings.check(&wrong()), Err(PairingProblem::BadToken));
    assert!(pairings.check(&next.token).is_ok(), "one wrong token does not end it");
}

#[test]
fn at_most_four_pairings_are_open() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let open: Vec<_> = (0..MAX_OPEN).map(|_| pairings.open(None).unwrap()).collect();

    assert_eq!(pairings.open(None).unwrap_err(), PairingProblem::TooMany);
    drop(open);
    assert!(pairings.open(None).is_ok());
}

#[tokio::test]
async fn a_pairing_whose_request_ends_tells_the_waiting_phone_refused() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();

    drop(opened);

    assert_eq!(wait.outcome().await, Answer::Ended(PairEnd::Refused));
    assert_eq!(pairings.open_count(), 0);
}

#[tokio::test]
async fn a_phone_that_leaves_before_the_answer_ends_the_pairing() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let mut opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();
    assert!(matches!(opened.next().await, Some(Update::Pending(_))));

    drop(wait);

    assert_eq!(opened.next().await, Some(Update::Ended(PairEnd::Expired)));
    assert_eq!(
        pairings.answer(id, true).unwrap_err(),
        PairingProblem::NotPending { pairing_id: id }
    );
}

#[tokio::test]
async fn a_phone_that_left_after_the_yes_gets_no_row() {
    let clock = TestClock::new();
    let store = TestStore::open(clock.shared()).await.unwrap();
    let pairings = pairings(&clock);
    let mut opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();
    assert!(matches!(opened.next().await, Some(Update::Pending(_))));
    let Decision::Accepted(accepted) = pairings.answer(id, true).unwrap() else {
        panic!("a yes accepts");
    };

    drop(wait);
    let completed = accepted.complete(store.writer().clone()).await;

    assert!(matches!(completed, Err(DaemonError::Pairing { problem: PairingProblem::PhoneLeft })));
    assert_eq!(opened.next().await, Some(Update::Ended(PairEnd::Expired)));
    let devices = store.readers().with(efr_store::devices::list).await.unwrap();
    assert!(devices.is_empty(), "{devices:?}");
}

#[test]
fn a_name_that_the_store_refuses_fails_before_anything_waits() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    for name in ["", "   ", "evil\u{202e}name", "line\nbreak", &"x".repeat(65)] {
        assert_eq!(
            pairings.open(Some(name.to_owned())).unwrap_err(),
            PairingProblem::BadField { field: "name" },
            "{name:?}"
        );
    }
    assert_eq!(pairings.open_count(), 0);

    let opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    assert_eq!(
        pairings.enroll(id, &opened.token, "bad\u{200b}name", key(), peer()).unwrap_err(),
        PairingProblem::BadField { field: "name" }
    );
    assert!(pairings.check(&opened.token).is_ok(), "a refused name does not spend the token");
}

#[test]
fn a_whois_fact_that_the_store_refuses_fails_before_the_terminal_hears_of_it() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let peer = EnrollPeer { node_stable_id: String::new(), ..peer() };

    assert_eq!(
        pairings.enroll(id, &opened.token, "iPhone", key(), peer).unwrap_err(),
        PairingProblem::BadField { field: "node_stable_id" }
    );
    assert!(pairings.check(&opened.token).is_ok());
}

#[test]
fn the_fingerprint_is_the_start_of_the_keys_sha256() {
    assert_eq!(key_fingerprint(&key()), "SHA256:0ed3a6ab957ff6f5");
}

#[test]
fn device_scopes_are_the_phone_scopes_without_admin() {
    assert_eq!(&DEVICE_SCOPES[..], granted(Origin::Phone, PeerSide::User));
    assert!(!DEVICE_SCOPES.contains(&ScopeName::Admin));
    assert!(!DEVICE_SCOPES.contains(&ScopeName::Terminal));
}

#[test]
fn same_compares_every_byte() {
    let a = [7; 32];
    let mut b = a;
    assert!(same(&a, &b));
    b[31] = 8;
    assert!(!same(&a, &b));
    b[31] = 7;
    b[0] = 0;
    assert!(!same(&a, &b));
}

#[test]
fn the_whois_facts_come_only_from_a_remote_admission() {
    let addr: SocketAddr = "100.64.0.2:51000".parse().unwrap();
    let remote = ConnectionContext::new(
        ConnId::new(1),
        Origin::Phone,
        RemotePeer::new(addr, Arc::new(crate::remote::test_admission(&peer()))),
    );
    let other = ConnectionContext::new(
        ConnId::new(2),
        Origin::Phone,
        RemotePeer::new(addr, Arc::new("not the facts")),
    );
    let local = ConnectionContext::new(ConnId::new(3), Origin::Cli, PeerCred::new(1000, Some(5)));

    // NOTE: the whois of efr-http has no operating system.
    assert_eq!(enroll_peer(&remote), Some(EnrollPeer { os: None, ..peer() }));
    assert_eq!(enroll_peer(&other), None);
    assert_eq!(enroll_peer(&local), None);
}

#[test]
fn a_device_id_comes_from_the_clock_and_the_generator() {
    let clock = TestClock::new();
    let pairings = pairings(&clock);
    let opened = pairings.open(None).unwrap();
    let id = pairings.check(&opened.token).unwrap();
    let _wait = pairings.enroll(id, &opened.token, "iPhone", key(), peer()).unwrap();

    let table = super::lock(&pairings.table);
    let Some(super::Entry { phase: super::Phase::Pending { device, .. }, .. }) =
        table.open.get(&id)
    else {
        panic!("the pairing waits for the terminal");
    };
    let device_id: DeviceId = device.device_id();
    assert_eq!(device_id.as_uuid().get_version_num(), 7);
}
