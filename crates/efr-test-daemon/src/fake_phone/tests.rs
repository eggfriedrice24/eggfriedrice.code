use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use efr_protocol::{
    Capabilities, DaemonId, DaemonPaths, DeviceId, ErrorCode, HelloResult, Method,
    PROTOCOL_VERSION, ProjectsList, ProveContext, ProveMessage,
};
use efr_test_support::tailscaled;
use pretty_assertions::assert_eq;

use super::{FakePhone, PhoneKey};
use crate::{TestDaemon, TestDaemonError};

fn device() -> DeviceId {
    "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap()
}

/// A hello result of the remote listener, as far as a proof reads it.
fn remote_hello() -> HelloResult {
    HelloResult {
        daemon_id: "0192f0c1-7a00-7000-8000-0000000000aa".parse::<DaemonId>().unwrap(),
        protocol: PROTOCOL_VERSION,
        version: "0.0.0".to_owned(),
        capabilities: Capabilities::default(),
        paths: DaemonPaths::default(),
        challenge: "c2FsdA".to_owned(),
        prove: Some(ProveContext {
            peer_node: tailscaled::PHONE_NODE.to_owned(),
            daemon_node: tailscaled::SELF_NODE.to_owned(),
            daemon_tailnet: tailscaled::TAILNET.to_owned(),
        }),
    }
}

#[test]
fn a_key_signs_the_prove_message_of_a_hello_in_der() {
    let key = PhoneKey::generate().unwrap();
    let public = key.public_key();
    assert_eq!(public.as_bytes().len(), 65);
    assert_eq!(public.as_bytes()[0], 0x04, "an uncompressed X9.63 point");

    let hello = remote_hello();
    let signature = key.sign(&hello, device()).unwrap();
    let message = ProveMessage {
        challenge: &hello.challenge,
        daemon_id: hello.daemon_id,
        protocol: hello.protocol,
        device_id: device(),
        peer_node: tailscaled::PHONE_NODE,
        daemon_tailnet: tailscaled::TAILNET,
    };
    let verifier = UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public.as_bytes());
    verifier.verify(&message.to_bytes(), signature.as_bytes()).unwrap();
    assert_eq!(signature.as_bytes()[0], 0x30, "a DER sequence");

    let other = PhoneKey::generate().unwrap();
    let foreign = other.sign(&hello, device()).unwrap();
    assert!(verifier.verify(&message.to_bytes(), foreign.as_bytes()).is_err());
}

#[test]
fn a_fingerprint_is_the_first_eight_bytes_of_the_hash_of_the_key() {
    let key = PhoneKey::generate().unwrap();
    let fingerprint = key.fingerprint();
    let hex = fingerprint.strip_prefix("SHA256:").unwrap();
    assert_eq!(hex.len(), 16);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_ne!(fingerprint, PhoneKey::generate().unwrap().fingerprint());
}

#[test]
fn a_hello_without_prove_facts_signs_nothing() {
    let mut hello = remote_hello();
    hello.prove = None;
    let error = PhoneKey::generate().unwrap().sign(&hello, device()).unwrap_err();
    assert!(matches!(error, TestDaemonError::NoProveFacts), "{error:?}");
}

#[tokio::test]
async fn a_phone_reaches_the_remote_listener_of_a_test_daemon() {
    let daemon = TestDaemon::builder().remote().start().await.unwrap();
    let addr = daemon.remote_addr().await.unwrap();
    assert!(addr.ip().is_loopback(), "a test daemon listens on loopback only");

    let mut phone = FakePhone::connect(addr).await.unwrap();
    let hello = phone.hello(None).await.unwrap();
    assert_eq!(hello.daemon_id, daemon.daemon_id());
    assert_eq!(hello.prove.unwrap().peer_node, tailscaled::PHONE_NODE);
    let early = phone.call::<serde_json::Value>(Method::ProjectsList(ProjectsList {}));
    let Err(TestDaemonError::PhoneServer { body }) = early.await else {
        panic!("a phone that did not prove reads nothing");
    };
    assert_eq!(body.code, ErrorCode::Unauthorized);

    let refused = FakePhone::connect_with_origin(addr, Some("http://evil.example")).await;
    let Err(TestDaemonError::PhoneRefused { status }) = refused else {
        panic!("another origin opens no socket");
    };
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");

    let fake = daemon.tailscaled().unwrap();
    assert!(fake.seen().iter().any(|target| target.starts_with("/localapi/v0/whois?")));
    fake.set_whois(None);
    assert_eq!(FakePhone::handshake(addr, None).await.unwrap(), "", "an unknown peer");
    drop(phone);
    daemon.stop().await.unwrap();
}
