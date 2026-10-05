use std::sync::Arc;
use std::sync::atomic::Ordering;

use efr_protocol::{ConversationId, Origin, PtyId, Seq};
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

/// The event that the notices decide on in the tests that are not about how far a
/// subscription got.
const EVENT: Seq = Seq::new(1);

#[test]
fn a_subscription_from_the_same_tty_attaches_until_its_guard_drops() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some(TTY)));

    let guard = connections.subscribe(ConnId::new(1), conversation(1));
    assert!(connections.attached(TTY, conversation(1), at(0), EVENT));
    assert!(!connections.attached(TTY, conversation(2), at(0), EVENT), "another conversation");
    assert!(!connections.attached("/dev/pts/4", conversation(1), at(0), EVENT), "another tty");

    drop(guard);
    assert!(!connections.attached(TTY, conversation(1), at(0), EVENT));
}

#[test]
fn a_subscription_from_a_connection_without_a_tty_attaches_nothing() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(None));
    let _guard = connections.subscribe(ConnId::new(1), conversation(1));

    assert!(!connections.attached(TTY, conversation(1), at(0), EVENT));
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

    assert!(connections.attached(TTY, conversation(1), at(59), EVENT));
    assert!(connections.watches_pty(pty, at(59)));
    assert!(!connections.attached(TTY, conversation(1), at(60), EVENT));
    assert!(!connections.watches_pty(pty, at(60)));
}

#[test]
fn a_closed_connection_attaches_nothing() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some(TTY)));
    let _guard = connections.subscribe(ConnId::new(1), conversation(1));

    connections.closed(ConnId::new(1));

    assert!(!connections.attached(TTY, conversation(1), at(0), EVENT));
    assert_eq!(connections.tty(ConnId::new(1)), None);
}

#[test]
fn an_ended_subscription_attaches_the_events_it_was_handed() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some(TTY)));
    let guard = connections.subscribe(ConnId::new(1), conversation(1));
    guard.reached().store(10, Ordering::Release);

    // efr shows the turn's last event, exits, and its connection goes before the
    // notices decide on that event.
    drop(guard);
    connections.closed(ConnId::new(1));

    assert!(connections.attached(TTY, conversation(1), at(0), Seq::new(9)));
    assert!(connections.attached(TTY, conversation(1), at(0), Seq::new(10)));
    assert!(!connections.attached(TTY, conversation(1), at(0), Seq::new(11)), "a later event");
    assert!(!connections.attached(TTY, conversation(2), at(0), Seq::new(10)), "another one");
    assert!(!connections.attached("/dev/pts/4", conversation(1), at(0), Seq::new(10)));
}

#[test]
fn the_furthest_ended_subscription_of_a_terminal_counts() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some(TTY)));
    connections.opened(ConnId::new(2), hello(Some(TTY)));
    let further = connections.subscribe(ConnId::new(1), conversation(1));
    let behind = connections.subscribe(ConnId::new(2), conversation(1));
    further.reached().store(20, Ordering::Release);
    behind.reached().store(5, Ordering::Release);

    drop(further);
    drop(behind);

    assert!(connections.attached(TTY, conversation(1), at(0), Seq::new(20)));
}

#[test]
fn an_ended_subscription_without_a_tty_or_an_event_attaches_nothing() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(None));
    connections.opened(ConnId::new(2), hello(Some(TTY)));
    let without_tty = connections.subscribe(ConnId::new(1), conversation(1));
    without_tty.reached().store(10, Ordering::Release);
    let without_event = connections.subscribe(ConnId::new(2), conversation(2));

    drop((without_tty, without_event));

    assert!(!connections.attached(TTY, conversation(1), at(0), Seq::new(1)));
    assert!(!connections.attached(TTY, conversation(2), at(0), Seq::new(1)));
}

#[test]
fn only_the_newest_ended_subscriptions_are_remembered() {
    let connections = Arc::new(Connections::default());
    connections.opened(ConnId::new(1), hello(Some(TTY)));
    let count = u128::try_from(super::MAX_FOLLOWED).unwrap() + 1;
    for n in 1..=count {
        let guard = connections.subscribe(ConnId::new(1), conversation(n));
        guard.reached().store(u64::try_from(n).unwrap(), Ordering::Release);
    }

    let seq = |n: u128| Seq::new(u64::try_from(n).unwrap());
    assert!(!connections.attached(TTY, conversation(1), at(0), seq(1)), "the oldest went");
    assert!(connections.attached(TTY, conversation(2), at(0), seq(2)));
    assert!(connections.attached(TTY, conversation(count), at(0), seq(count)));
}
