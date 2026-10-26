use std::net::Ipv4Addr;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
use efr_protocol::{DeviceId, ScopeName};
use efr_store::Store;
use efr_store::devices::{Device, DevicePeer, DevicePublicKey, NewDevice};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use super::{check, scopes, verify};
use crate::DaemonError;
use crate::remote::admission::{Admission, PeerFacts};
use crate::remote::tailnet::Tailnet;

fn key() -> EcdsaKeyPair {
    EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap()
}

fn sign(key: &EcdsaKeyPair, message: &[u8]) -> Vec<u8> {
    key.sign(&SystemRandom::new(), message).unwrap().as_ref().to_vec()
}

#[test]
fn a_der_signature_of_the_key_passes_and_every_other_fails() {
    let key = key();
    let public = key.public_key().as_ref().to_vec();
    assert_eq!(public.len(), 65, "the uncompressed point");
    let signature = sign(&key, b"message");

    assert!(verify(&public, b"message", &signature).is_ok());
    assert!(matches!(verify(&public, b"other", &signature), Err(DaemonError::ProofRefused)));
    let other = self::key();
    let foreign = sign(&other, b"message");
    assert!(verify(&public, b"message", &foreign).is_err(), "another key's signature");
    assert!(verify(&public, b"message", &signature[..signature.len() - 1]).is_err(), "cut DER");
    assert!(verify(&public, b"message", &[]).is_err());

    // A point that is not on the curve is refused, though its shape is right.
    let mut off_curve = vec![4_u8];
    off_curve.extend([1; 64]);
    assert!(verify(&off_curve, b"message", &signature).is_err());
}

#[test]
fn a_proved_connection_gets_the_phone_scopes_of_its_device_and_no_more() {
    let all = [ScopeName::Read, ScopeName::Operate, ScopeName::Approve, ScopeName::Terminal];
    assert_eq!(scopes(&all), [ScopeName::Read, ScopeName::Operate, ScopeName::Approve]);
    assert_eq!(scopes(&[ScopeName::Read]), [ScopeName::Read]);
    assert_eq!(scopes(&[ScopeName::Admin, ScopeName::Terminal]), Vec::<ScopeName>::new());
}

fn tailnet() -> Tailnet {
    Tailnet {
        ipv4: Ipv4Addr::new(100, 101, 102, 103),
        own_ips: Vec::new(),
        self_node: "nSELF".to_owned(),
        self_user: Some(42),
        tailnet: "T1".to_owned(),
        fqdn: None,
    }
}

fn admission(node: &str, user: i64, tailnet: &Tailnet) -> Admission {
    let peer = PeerFacts {
        node_stable_id: node.to_owned(),
        node_name: "phone".to_owned(),
        host_name: "phone".to_owned(),
        user_id: user,
        login_name: "user@example.com".to_owned(),
        display_name: "User".to_owned(),
    };
    Admission::new(1, peer, tailnet, CancellationToken::new())
}

/// The row of a device with `key`, paired from node `nPHONE` of user 42 on `T1`;
/// revoked when `revoked`.
async fn row(key: &EcdsaKeyPair, revoked: bool) -> Device {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let device_id = DeviceId::from_uuid(uuid::Uuid::from_u128(1));
    let peer = DevicePeer {
        node_stable_id: "nPHONE".to_owned(),
        user_id: 42,
        tailnet_stable_id: "T1".to_owned(),
    };
    let public = DevicePublicKey::from_x963(key.public_key().as_ref()).unwrap();
    let new =
        NewDevice::new(device_id, "phone", public, "SHA256:abc", peer, &[ScopeName::Read]).unwrap();
    store.writer().device_insert(new).await.unwrap();
    if revoked {
        store.writer().device_revoke(device_id).await.unwrap();
    }
    let row = store
        .readers()
        .with(move |conn| efr_store::devices::get(conn, device_id))
        .await
        .unwrap()
        .unwrap();
    store.close().await;
    row
}

#[tokio::test]
async fn the_row_must_be_active_and_of_the_same_user_tailnet_and_node() {
    let key = key();
    let signature = sign(&key, b"m");
    let active = row(&key, false).await;
    let here = tailnet();

    assert!(check(&active, &admission("nPHONE", 42, &here), b"m", &signature).is_ok());
    let refused = [
        (admission("nPHONE", 43, &here), "another user"),
        (
            admission("nPHONE", 42, &Tailnet { tailnet: "T2".to_owned(), ..tailnet() }),
            "another tailnet",
        ),
    ];
    for (admission, why) in refused {
        let checked = check(&active, &admission, b"m", &signature);
        assert!(matches!(checked, Err(DaemonError::ProofRefused)), "{why}");
    }
    let moved = check(&active, &admission("nNEWPHONE", 42, &here), b"m", &signature);
    assert!(matches!(moved, Err(DaemonError::RepairNeeded { .. })), "a new node pairs again");

    let revoked = row(&key, true).await;
    let checked = check(&revoked, &admission("nPHONE", 42, &here), b"m", &signature);
    assert!(matches!(checked, Err(DaemonError::ProofRefused)), "a revoked device");

    // NOTE: the signature comes first, so a peer without the key never learns that a
    // device moved.
    let wrong = check(&active, &admission("nNEWPHONE", 42, &here), b"m", &[0x30, 0x00]);
    assert!(matches!(wrong, Err(DaemonError::ProofRefused)));
}
