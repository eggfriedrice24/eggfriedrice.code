use std::net::{Ipv4Addr, SocketAddr};

use efr_protocol::{DeviceId, ProveContext, ScopeName};
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use super::{Admission, PeerFacts, Proved, Refusal, Whois, decide, refuse_before_whois};
use crate::remote::tailnet::Tailnet;

fn tailnet() -> Tailnet {
    Tailnet {
        ipv4: Ipv4Addr::new(100, 101, 102, 103),
        own_ips: vec!["100.101.102.103".parse().unwrap(), "fd7a:115c:a1e0::1".parse().unwrap()],
        self_node: "nSELF".to_owned(),
        self_user: Some(42),
        tailnet: "T1".to_owned(),
        fqdn: Some("desk.example.ts.net".to_owned()),
    }
}

fn phone() -> Whois {
    Whois {
        node_stable_id: "nPHONE".to_owned(),
        node_name: "phone.example.ts.net.".to_owned(),
        host_name: "phone".to_owned(),
        tags: Vec::new(),
        sharee_node: false,
        user_id: 42,
        login_name: "user@example.com".to_owned(),
        display_name: "User".to_owned(),
    }
}

#[test]
fn a_node_of_the_owner_is_admitted_with_its_facts() {
    let facts = decide(&tailnet(), Some(phone())).unwrap();
    assert_eq!(
        facts,
        PeerFacts {
            node_stable_id: "nPHONE".to_owned(),
            node_name: "phone.example.ts.net".to_owned(),
            host_name: "phone".to_owned(),
            user_id: 42,
            login_name: "user@example.com".to_owned(),
            display_name: "User".to_owned(),
        }
    );
}

#[test]
fn each_peer_that_is_not_the_owners_own_phone_is_refused() {
    let self_node = Whois { node_stable_id: "nSELF".to_owned(), ..phone() };
    let tagged = Whois { tags: vec!["tag:ci".to_owned()], ..phone() };
    let shared = Whois { sharee_node: true, ..phone() };
    let other = Whois { user_id: 43, ..phone() };
    let cases = [
        (None, Refusal::UnknownPeer),
        (Some(self_node), Refusal::SelfNode),
        (Some(tagged), Refusal::Tagged),
        (Some(shared), Refusal::Shared),
        (Some(other), Refusal::OtherUser),
    ];
    for (whois, refusal) in cases {
        assert_eq!(decide(&tailnet(), whois), Err(refusal), "{refusal:?}");
    }
    let tagged_daemon = Tailnet { self_user: None, ..tailnet() };
    assert_eq!(decide(&tagged_daemon, Some(phone())), Err(Refusal::TaggedDaemon));
}

#[test]
fn an_address_of_this_node_is_refused_before_any_whois() {
    let refused: [SocketAddr; 3] = [
        "100.101.102.103:50000".parse().unwrap(),
        "[::ffff:100.101.102.103]:50000".parse().unwrap(),
        "[fd7a:115c:a1e0::1]:50000".parse().unwrap(),
    ];
    for peer in refused {
        assert_eq!(refuse_before_whois(&tailnet(), peer), Some(Refusal::OwnAddress), "{peer}");
    }
    let phone: SocketAddr = "100.64.0.9:50000".parse().unwrap();
    assert_eq!(refuse_before_whois(&tailnet(), phone), None);
}

#[test]
fn the_admission_keeps_the_challenge_until_a_proof_spends_it() {
    let facts = decide(&tailnet(), Some(phone())).unwrap();
    let admission = Admission::new(7, facts, &tailnet(), CancellationToken::new());
    assert_eq!(
        admission.prove_context(),
        ProveContext {
            peer_node: "nPHONE".to_owned(),
            daemon_node: "nSELF".to_owned(),
            daemon_tailnet: "T1".to_owned(),
        }
    );
    assert_eq!(admission.challenge(), None, "no hello yet");
    let device = DeviceId::from_uuid(uuid::Uuid::from_u128(1));
    admission.hello("abc", Some(device));
    assert_eq!(admission.challenge(), Some(("abc".to_owned(), Some(device))));

    assert_eq!(admission.fail(), 1);
    assert_eq!(admission.fail(), 2);
    let proved = Proved { device_id: device, scopes: vec![ScopeName::Read] };
    assert!(admission.prove(proved.clone()));
    assert_eq!(admission.proved(), Some(&proved));
    assert_eq!(admission.challenge(), None, "the challenge is spent");
    assert!(!admission.prove(proved), "a connection proves one device once");
}
