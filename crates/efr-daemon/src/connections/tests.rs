use std::sync::Arc;
use std::sync::atomic::Ordering;

use efr_protocol::{ConversationId, Origin, PtyId, Seq};
use efr_transport::ConnId;
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use crate::connections::{Connections, Decision, HelloInfo, Lease, Notice};

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

/// The notice for the event `seq` of `conversation` in `tty`.
fn notice_in(tty: &str, conversation: ConversationId, seq: u64) -> Notice {
    Notice {
        tty: tty.to_owned(),
        conversation,
        seq: Seq::new(seq),
        text: format!("efr: turn finished: {seq}"),
    }
}

fn notice(conversation: ConversationId, seq: u64) -> Notice {
    notice_in(TTY, conversation, seq)
}

fn connections_with(conns: &[(u64, Option<&str>)]) -> Arc<Connections> {
    let connections = Arc::new(Connections::default());
    for (n, tty) in conns {
        connections.opened(ConnId::new(*n), hello(*tty));
    }
    connections
}

#[test]
fn a_notice_that_nobody_in_its_terminal_can_show_is_written_at_once() {
    let connections = connections_with(&[(1, Some("/dev/pts/4")), (2, None), (3, Some(TTY))]);
    let _elsewhere = connections.subscribe(ConnId::new(1), conversation(1), false);
    let _without_tty = connections.subscribe(ConnId::new(2), conversation(1), false);
    let _other_conversation = connections.subscribe(ConnId::new(3), conversation(2), false);

    let decision = connections.decide(notice(conversation(1), 5), at(0));

    assert_eq!(decision, Decision::Write(notice(conversation(1), 5)));
    assert!(connections.take_ready().is_empty());
}

#[test]
fn an_open_subscription_that_was_handed_the_event_shows_it() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);
    guard.reached().store(5, Ordering::Release);

    assert_eq!(connections.decide(notice(conversation(1), 5), at(0)), Decision::Shown);
}

#[test]
fn an_open_subscription_holds_the_notice_until_it_ends_and_then_it_shows_it() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);

    assert_eq!(connections.decide(notice(conversation(1), 5), at(0)), Decision::Held);
    guard.reached().store(5, Ordering::Release);
    drop(guard);
    connections.closed(ConnId::new(1));

    assert!(connections.take_ready().is_empty(), "the view was handed the event");
}

#[test]
fn a_held_notice_is_ready_when_the_subscription_ends_before_the_event() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);
    guard.reached().store(4, Ordering::Release);

    assert_eq!(connections.decide(notice(conversation(1), 5), at(0)), Decision::Held);
    drop(guard);

    assert_eq!(connections.take_ready(), vec![notice(conversation(1), 5)]);
    assert!(connections.take_ready().is_empty(), "a ready notice is taken once");
}

#[test]
fn an_ended_subscription_shows_the_events_it_was_handed() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);
    guard.reached().store(10, Ordering::Release);

    // efr shows the turn's last event, exits, and its connection goes before the
    // notices decide on that event.
    drop(guard);
    connections.closed(ConnId::new(1));

    for seq in [9, 10] {
        assert_eq!(connections.decide(notice(conversation(1), seq), at(0)), Decision::Shown);
    }
    let later = notice(conversation(1), 11);
    assert_eq!(connections.decide(later.clone(), at(0)), Decision::Write(later));
    let another = notice(conversation(2), 10);
    assert_eq!(connections.decide(another.clone(), at(0)), Decision::Write(another));
    let elsewhere = notice_in("/dev/pts/4", conversation(1), 10);
    assert_eq!(connections.decide(elsewhere.clone(), at(0)), Decision::Write(elsewhere));
}

#[test]
fn the_furthest_ended_subscription_of_a_terminal_counts() {
    let connections = connections_with(&[(1, Some(TTY)), (2, Some(TTY))]);
    let further = connections.subscribe(ConnId::new(1), conversation(1), false);
    let behind = connections.subscribe(ConnId::new(2), conversation(1), false);
    further.reached().store(20, Ordering::Release);
    behind.reached().store(5, Ordering::Release);

    drop(further);
    drop(behind);

    assert_eq!(connections.decide(notice(conversation(1), 20), at(0)), Decision::Shown);
}

#[test]
fn a_subscription_without_a_tty_or_an_event_shows_nothing() {
    let connections = connections_with(&[(1, None), (2, Some(TTY))]);
    let without_tty = connections.subscribe(ConnId::new(1), conversation(1), false);
    without_tty.reached().store(10, Ordering::Release);
    let without_event = connections.subscribe(ConnId::new(2), conversation(2), false);

    drop((without_tty, without_event));

    for n in [1, 2] {
        let notice = notice(conversation(n), 1);
        assert_eq!(connections.decide(notice.clone(), at(0)), Decision::Write(notice));
    }
}

#[test]
fn a_prompt_holds_the_notices_of_its_terminal_until_its_connection_closes() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let mut prompting = connections.prompting(ConnId::new(1));
    // The turn ends before the prompt answers: its conversation is not known yet.
    assert_eq!(connections.decide(notice(conversation(1), 3), at(0)), Decision::Held);
    prompting.sent_to(conversation(1));
    drop(prompting);
    assert_eq!(connections.decide(notice(conversation(1), 4), at(0)), Decision::Held);
    assert!(connections.take_ready().is_empty(), "the connection may still follow");
    let other = notice(conversation(2), 5);
    assert_eq!(connections.decide(other.clone(), at(0)), Decision::Write(other));

    connections.closed(ConnId::new(1));

    let ready = connections.take_ready();
    assert_eq!(ready, vec![notice(conversation(1), 3), notice(conversation(1), 4)]);
}

#[test]
fn a_turn_that_ended_before_the_view_subscribed_gets_no_notice_once_shown() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let mut prompting = connections.prompting(ConnId::new(1));
    assert_eq!(connections.decide(notice(conversation(1), 7), at(0)), Decision::Held);
    prompting.sent_to(conversation(1));
    drop(prompting);

    // efr subscribes after the end, is handed it in the replay, shows it and leaves.
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);
    guard.reached().store(7, Ordering::Release);
    drop(guard);
    connections.closed(ConnId::new(1));

    assert!(connections.take_ready().is_empty());
}

#[test]
fn a_prompt_that_was_not_sent_holds_only_until_it_answers() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let prompting = connections.prompting(ConnId::new(1));
    assert_eq!(connections.decide(notice(conversation(1), 3), at(0)), Decision::Held);

    drop(prompting);

    assert_eq!(connections.take_ready(), vec![notice(conversation(1), 3)]);
}

#[test]
fn a_prompt_from_a_connection_without_a_hello_or_a_tty_holds_nothing() {
    let connections = connections_with(&[(2, None)]);
    let _without_hello = connections.prompting(ConnId::new(1));
    let _without_tty = connections.prompting(ConnId::new(2));

    let notice = notice(conversation(1), 3);
    assert_eq!(connections.decide(notice.clone(), at(0)), Decision::Write(notice));
}

#[tokio::test]
async fn a_notice_that_becomes_ready_wakes_the_waiter() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);
    assert_eq!(connections.decide(notice(conversation(1), 5), at(0)), Decision::Held);

    drop(guard);

    // NOTE: the wake was sent before anyone waited, so the wait completes at once.
    connections.readied().await;
    assert_eq!(connections.take_ready(), vec![notice(conversation(1), 5)]);
}

#[test]
fn a_view_that_stays_open_keeps_only_the_notices_it_was_not_handed_yet() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);
    for seq in 1..=200 {
        assert_eq!(connections.decide(notice(conversation(1), seq), at(0)), Decision::Held);
        guard.reached().store(seq, Ordering::Release);
    }
    guard.reached().store(199, Ordering::Release);

    drop(guard);

    assert_eq!(connections.take_ready(), vec![notice(conversation(1), 200)]);
}

#[test]
fn at_most_the_newest_held_notices_wait() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), false);
    let count = u64::try_from(super::MAX_HELD).unwrap() + 1;
    for seq in 1..=count {
        connections.decide(notice(conversation(1), seq), at(0));
    }

    drop(guard);

    let ready = connections.take_ready();
    assert_eq!(ready.len(), super::MAX_HELD);
    assert_eq!(ready.first().map(|notice| notice.seq), Some(Seq::new(2)), "the oldest went");
}

#[test]
fn a_lease_shows_until_it_expires() {
    let connections = connections_with(&[(1, Some(TTY))]);
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

    assert_eq!(connections.decide(notice(conversation(1), 1), at(59)), Decision::Shown);
    assert!(connections.watches_pty(pty, at(59)));
    let expired = notice(conversation(1), 1);
    assert_eq!(connections.decide(expired.clone(), at(60)), Decision::Write(expired));
    assert!(!connections.watches_pty(pty, at(60)));
}

#[test]
fn a_closed_connection_forgets_its_tty() {
    let connections = connections_with(&[(1, Some(TTY))]);
    connections.closed(ConnId::new(1));
    assert_eq!(connections.tty(ConnId::new(1)), None);
}

#[test]
fn only_the_newest_ended_subscriptions_are_remembered() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let count = u128::try_from(super::MAX_FOLLOWED).unwrap() + 1;
    for n in 1..=count {
        let guard = connections.subscribe(ConnId::new(1), conversation(n), false);
        guard.reached().store(u64::try_from(n).unwrap(), Ordering::Release);
    }

    let seq = |n: u128| u64::try_from(n).unwrap();
    let oldest = notice(conversation(1), seq(1));
    assert_eq!(connections.decide(oldest.clone(), at(0)), Decision::Write(oldest));
    assert_eq!(connections.decide(notice(conversation(2), seq(2)), at(0)), Decision::Shown);
    let newest = notice(conversation(count), seq(count));
    assert_eq!(connections.decide(newest, at(0)), Decision::Shown);
}

#[test]
fn answering_subscriptions_are_counted_per_conversation_until_their_guards_drop() {
    let connections = connections_with(&[(1, Some(TTY)), (2, None)]);
    assert_eq!(connections.answerers(conversation(1)), 0);

    let watching = connections.subscribe(ConnId::new(1), conversation(1), false);
    assert_eq!(connections.answerers(conversation(1)), 0, "a viewer cannot answer");
    let first = connections.subscribe(ConnId::new(1), conversation(1), true);
    let second = connections.subscribe(ConnId::new(2), conversation(1), true);
    let other = connections.subscribe(ConnId::new(2), conversation(2), true);
    assert_eq!(connections.answerers(conversation(1)), 2);
    assert_eq!(connections.answerers(conversation(2)), 1);

    drop(first);
    assert_eq!(connections.answerers(conversation(1)), 1);
    drop((second, watching, other));
    assert_eq!(connections.answerers(conversation(1)), 0);
    assert_eq!(connections.answerers(conversation(2)), 0);
}

#[test]
fn a_closed_connection_answers_nothing_even_before_its_requests_end() {
    let connections = connections_with(&[(1, Some(TTY))]);
    let guard = connections.subscribe(ConnId::new(1), conversation(1), true);
    connections.closed(ConnId::new(1));
    assert_eq!(connections.answerers(conversation(1)), 0);
    drop(guard);
    assert_eq!(connections.answerers(conversation(1)), 0);
}

#[test]
fn a_subscription_without_a_hello_never_counts() {
    let connections = Arc::new(Connections::default());
    let _guard = connections.subscribe(ConnId::new(7), conversation(1), true);
    assert_eq!(connections.answerers(conversation(1)), 0);
    let notice = notice(conversation(1), 1);
    assert_eq!(connections.decide(notice.clone(), at(0)), Decision::Write(notice));
}
