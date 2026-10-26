use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use efr_protocol::Origin;
use pretty_assertions::assert_eq;

use super::{ConnId, ConnectionContext, Peer, PeerCred, RemotePeer};

/// What a daemon could keep about a remote peer: fixed facts and a value set later.
#[derive(Debug)]
struct Admission {
    node: &'static str,
    device: OnceLock<u32>,
}

fn addr() -> SocketAddr {
    "100.64.0.7:41234".parse().unwrap()
}

#[test]
fn a_local_peer_has_the_kernel_credentials_and_its_declared_surface() {
    let context = ConnectionContext::new(ConnId::new(3), Origin::Cli, PeerCred::new(1000, Some(7)));
    assert_eq!(context.surface(), Origin::Cli);
    assert_eq!((context.uid(), context.pid()), (Some(1000), Some(7)));
    assert_eq!(context.peer(), &Peer::Local(PeerCred::new(1000, Some(7))));
    assert!(context.remote().is_none());
}

#[test]
fn a_remote_peer_is_always_a_phone_and_has_no_uid_or_pid() {
    let admission = Arc::new(Admission { node: "nKr7", device: OnceLock::new() });
    let remote = RemotePeer::new(addr(), admission);
    for declared in [Origin::Cli, Origin::Shell, Origin::Proxy, Origin::Phone] {
        let context = ConnectionContext::new(ConnId::new(4), declared, remote.clone());
        assert_eq!(context.surface(), Origin::Phone, "declared {declared:?}");
        assert_eq!((context.uid(), context.pid()), (None, None));
        assert_eq!(context.remote().map(RemotePeer::addr), Some(addr()));
    }
}

#[test]
fn the_admission_value_is_shared_by_every_clone_and_read_back_by_type() {
    let admission = Arc::new(Admission { node: "nKr7", device: OnceLock::new() });
    let context =
        ConnectionContext::new(ConnId::new(5), Origin::Phone, RemotePeer::new(addr(), admission));
    let copy = context.clone();
    let remote = context.remote().unwrap();
    assert_eq!(remote.admission::<Admission>().unwrap().node, "nKr7");
    assert!(remote.admission::<String>().is_none());
    remote.admission::<Admission>().unwrap().device.set(9).unwrap();
    assert_eq!(copy.remote().unwrap().admission::<Admission>().unwrap().device.get(), Some(&9));
    assert_eq!(copy, context);
}

#[test]
fn two_remote_peers_with_their_own_admission_values_differ() {
    let first = RemotePeer::new(addr(), Arc::new(1_u8));
    let second = RemotePeer::new(addr(), Arc::new(1_u8));
    assert_ne!(first, second);
    assert_eq!(first.clone(), first);
}

#[test]
fn the_debug_form_of_a_remote_peer_shows_the_address_only() {
    let remote = RemotePeer::new(addr(), Arc::new("a secret the daemon keeps"));
    let text = format!("{remote:?}");
    assert!(text.contains("100.64.0.7:41234"), "{text}");
    assert!(!text.contains("secret"), "{text}");
}

#[test]
fn every_new_connection_id_is_unique() {
    let ids: Vec<ConnId> = (0..64).map(|_| ConnId::next()).collect();
    for pair in ids.windows(2) {
        assert!(pair[0] < pair[1], "{pair:?}");
    }
}
