use std::sync::Arc;

use efr_protocol::{ConversationId, Origin, PtyId};
use efr_transport::ConnId;
use jiff::Timestamp;

use crate::connections::{Connections, HelloInfo, Lease};

const TTY: &str = "/dev/pts/3";

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn hello(tty: Option<&str>) -> HelloInfo {
    HelloInfo { surface: Origin::Shell, tty: tty.map(str::to_owned), client: None }
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + seconds).unwrap()
}

#[test]
fn a_subscription_from_the_same_tty_attaches_until_its_guard_drops() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some(TTY)));

    let guard = connections.subscribe(ConnId::new(1), conversation(1));
    assert!(connections.attached(TTY, conversation(1), at(0)));
    assert!(!connections.attached(TTY, conversation(2), at(0)), "another conversation");
    assert!(!connections.attached("/dev/pts/4", conversation(1), at(0)), "another tty");

    drop(guard);
    assert!(!connections.attached(TTY, conversation(1), at(0)));
}

#[test]
fn a_subscription_from_a_connection_without_a_tty_attaches_nothing() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(None));
    let _guard = connections.subscribe(ConnId::new(1), conversation(1));

    assert!(!connections.attached(TTY, conversation(1), at(0)));
}

#[test]
fn a_lease_attaches_until_it_expires() {
    let connections = Connections::default();
    connections.opened(ConnId::new(1), hello(Some(TTY)));
    let pty = PtyId::from_uuid(uuid::Uuid::from_u128(9));
    connections.report_lease(
        ConnId::new(1),
        Lease {
            conversations: vec![conversation(1)],
            ptys: vec![pty],
            visible: true,
            expires_at: at(60),
        },
    );

    assert!(connections.attached(TTY, conversation(1), at(59)));
    assert!(connections.watches_pty(pty, at(59)));
    assert!(!connections.attached(TTY, conversation(1), at(60)));
    assert!(!connections.watches_pty(pty, at(60)));
}

#[test]
fn a_closed_connection_attaches_nothing() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some(TTY)));
    let _guard = connections.subscribe(ConnId::new(1), conversation(1));

    connections.closed(ConnId::new(1));

    assert!(!connections.attached(TTY, conversation(1), at(0)));
    assert_eq!(connections.tty(ConnId::new(1)), None);
}
