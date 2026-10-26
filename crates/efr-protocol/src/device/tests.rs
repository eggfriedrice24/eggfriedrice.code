use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;
use uuid::Uuid;

use super::{DeviceInfo, PROVE_DOMAIN, PairEnd, PairingQr, ProveContext, ProveMessage};
use crate::{DaemonId, DeviceId, SecretText};

fn nil_message<'a>(challenge: &'a str, peer_node: &'a str, tailnet: &'a str) -> ProveMessage<'a> {
    ProveMessage {
        challenge,
        daemon_id: DaemonId::from_uuid(Uuid::nil()),
        protocol: 1,
        device_id: DeviceId::from_uuid(Uuid::nil()),
        peer_node,
        daemon_tailnet: tailnet,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The vector was computed outside Rust, from the rule in the doc comment: seven
/// fields, each a 4-byte big-endian length and its UTF-8 bytes.
#[test]
fn the_message_is_the_seven_length_prefixed_fields_in_order() {
    let bytes = nil_message("abc", "n1", "T1").to_bytes();
    assert_eq!(bytes.len(), 127);
    assert_eq!(
        hex(&bytes),
        "000000136566722d6465766963652d70726f76652d763100000003616263000000243030303030303030\
         2d303030302d303030302d303030302d30303030303030303030303000000001310000002430303030\
         303030302d303030302d303030302d303030302d303030303030303030303030000000026e31000000\
         025431"
    );
}

#[test]
fn the_domain_tag_names_the_version() {
    assert_eq!(PROVE_DOMAIN, "efr-device-prove-v1");
}

#[test]
fn each_field_changes_the_message() {
    let base = nil_message("abc", "n1", "T1");
    let other_id = Uuid::from_u128(1);
    let variants = [
        nil_message("abx", "n1", "T1"),
        nil_message("abc", "n2", "T1"),
        nil_message("abc", "n1", "T2"),
        ProveMessage { daemon_id: DaemonId::from_uuid(other_id), ..base },
        ProveMessage { device_id: DeviceId::from_uuid(other_id), ..base },
        ProveMessage { protocol: 2, ..base },
    ];
    for variant in variants {
        assert_ne!(variant.to_bytes(), base.to_bytes(), "{variant:?}");
    }
}

proptest! {
    /// The lengths keep the fields apart: moving a byte from one field to the next
    /// never gives the same message.
    #[test]
    fn fields_never_run_into_each_other(
        challenge in "[A-Za-z0-9_-]{0,8}",
        peer in "[A-Za-z0-9]{0,8}",
        tailnet in "[A-Za-z0-9]{0,8}",
        cut in 0usize..8,
    ) {
        let joined = format!("{peer}{tailnet}");
        let cut = cut.min(joined.len());
        let (left, right) = joined.split_at(cut);
        let one = nil_message(&challenge, &peer, &tailnet).to_bytes();
        let two = nil_message(&challenge, left, right).to_bytes();
        prop_assert_eq!(one == two, left == peer);
    }
}

fn qr(fqdn: Option<&str>, token: &str) -> PairingQr {
    PairingQr {
        daemon_id: DaemonId::from_uuid(Uuid::from_u128(0x019a_9b1c_3d00_7a10_8b20_0000_0000_0007)),
        host: "100.64.0.1".parse().unwrap(),
        port: 7878,
        fqdn: fqdn.map(str::to_owned),
        token: SecretText::new(token),
    }
}

#[test]
fn the_qr_url_lists_its_members_in_order() {
    let url = qr(Some("desk.tail1234.ts.net"), "q83vASNFZ4mrze8BI0VniQ").url();
    assert_eq!(
        url.expose_secret(),
        "efr://pair?v=1&d=019a9b1c-3d00-7a10-8b20-000000000007&h=100.64.0.1&p=7878\
         &f=desk.tail1234.ts.net&t=q83vASNFZ4mrze8BI0VniQ"
    );
}

#[test]
fn the_qr_url_leaves_out_an_absent_fqdn_and_encodes_unsafe_bytes() {
    let url = qr(None, "a b&c=d/é").url();
    assert_eq!(
        url.expose_secret(),
        "efr://pair?v=1&d=019a9b1c-3d00-7a10-8b20-000000000007&h=100.64.0.1&p=7878\
         &t=a%20b%26c%3Dd%2F%C3%A9"
    );
}

#[test]
fn the_qr_url_is_redacted_in_debug() {
    let url = qr(None, "q83vASNFZ4mrze8BI0VniQ").url();
    assert!(!format!("{url:?}").contains("q83v"));
    assert!(!format!("{:?}", qr(None, "q83vASNFZ4mrze8BI0VniQ")).contains("q83v"));
}

#[test]
fn the_qr_payload_on_the_wire() {
    let wire = json!({
        "daemon_id": "019a9b1c-3d00-7a10-8b20-000000000007",
        "host": "100.64.0.1",
        "port": 7878,
        "token": "tok",
    });
    assert_eq!(serde_json::to_value(qr(None, "tok")).unwrap(), wire);
    assert_eq!(serde_json::from_value::<PairingQr>(wire).unwrap(), qr(None, "tok"));
}

#[test]
fn a_qr_payload_with_a_host_that_is_not_ipv4_does_not_read() {
    let wire = json!({
        "daemon_id": "019a9b1c-3d00-7a10-8b20-000000000007",
        "host": "desk.tail1234.ts.net",
        "port": 7878,
        "token": "tok",
    });
    assert!(serde_json::from_value::<PairingQr>(wire).is_err());
}

#[test]
fn the_prove_context_on_the_wire() {
    let context = ProveContext {
        peer_node: "nP".into(),
        daemon_node: "nD".into(),
        daemon_tailnet: "T".into(),
    };
    let wire = json!({ "peer_node": "nP", "daemon_node": "nD", "daemon_tailnet": "T" });
    assert_eq!(serde_json::to_value(&context).unwrap(), wire);
    assert_eq!(serde_json::from_value::<ProveContext>(wire).unwrap(), context);
}

#[test]
fn a_device_that_never_connected_and_is_not_revoked_leaves_both_times_out() {
    let wire = json!({
        "device_id": "019a9b1c-3d00-7a10-8b20-000000000006",
        "name": "phone",
        "key_fingerprint": "SHA256:00",
        "node": "nP",
        "scopes": ["read"],
        "created_at": "2026-10-26T09:01:00Z",
    });
    let info: DeviceInfo = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!((info.last_seen_at, info.revoked_at), (None, None));
    assert_eq!(serde_json::to_value(&info).unwrap(), wire);
}

#[test]
fn pair_ends_on_the_wire() {
    let names: Vec<String> = PairEnd::ALL
        .iter()
        .map(|end| serde_json::to_value(end).unwrap().as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["refused", "expired", "attempts", "unknown"]);
}
