//! Presence over a `TestDaemon`: a conversation goes away after the idle minutes and
//! comes back with the next input, and `presence.set` sets it as the user asks, for one
//! conversation or for each one that is present.

use std::time::Duration;

use efr_protocol::{
    ConversationId, Event, Method, Presence, PresenceSet, PresenceSetResult, PresenceSource,
    PromptSendResult,
};
use efr_test_daemon::{Client, TestDaemon, command_id};
use pretty_assertions::assert_eq;

/// Sends a prompt with the command id `n` from `tty` and waits until its turn ends.
async fn prompt(daemon: &TestDaemon, client: &Client, n: u128, tty: &str) -> PromptSendResult {
    let sent: PromptSendResult = client.call(daemon.prompt(n, "hello", tty)).await.unwrap();
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
    efr_test_daemon::events_until(&mut follow, |event| {
        matches!(event, Event::TurnFailed { turn_id, .. } | Event::TurnCompleted { turn_id, .. } if *turn_id == sent.turn_id)
    })
    .await
    .unwrap();
    sent
}

/// The presence changes of `conversation_id`, in order.
async fn changes(
    daemon: &TestDaemon,
    client: &Client,
    conversation_id: ConversationId,
) -> Vec<(Presence, PresenceSource)> {
    daemon
        .events(client, conversation_id)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|envelope| match envelope.event {
            Event::PresenceChanged { presence, source, .. } => Some((presence, source)),
            _ => None,
        })
        .collect()
}

fn set(n: u128, conversation_id: Option<ConversationId>, presence: Presence) -> Method {
    Method::PresenceSet(PresenceSet { command_id: command_id(n), conversation_id, presence })
}

#[tokio::test]
async fn presence_away_after_idle_minutes() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let first = prompt(&daemon, &client, 1, "/dev/pts/1").await;

    daemon.clock().advance(Duration::from_secs(9 * 60));
    let second = prompt(&daemon, &client, 2, "/dev/pts/1").await;
    assert_eq!(second.conversation_id, first.conversation_id);
    assert_eq!(changes(&daemon, &client, first.conversation_id).await, [], "9 minutes is not idle");

    daemon.clock().advance(Duration::from_secs(11 * 60));
    prompt(&daemon, &client, 3, "/dev/pts/1").await;

    assert_eq!(
        changes(&daemon, &client, first.conversation_id).await,
        [(Presence::Away, PresenceSource::Timeout), (Presence::Present, PresenceSource::Input)],
        "the input after 11 idle minutes records the idle state, then the return"
    );
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn presence_set_by_user() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let sent = prompt(&daemon, &client, 1, "/dev/pts/1").await;
    let id = sent.conversation_id;

    let away: PresenceSetResult = client.call(set(2, Some(id), Presence::Away)).await.unwrap();
    let again: PresenceSetResult = client.call(set(2, Some(id), Presence::Away)).await.unwrap();
    let twice: PresenceSetResult = client.call(set(3, Some(id), Presence::Away)).await.unwrap();

    assert_eq!(away.presence, Presence::Away);
    assert_eq!(away.changed, [id]);
    assert!(away.seq.is_some());
    assert_eq!(again, away, "a retried command answers from its receipt");
    assert_eq!(twice.changed, [], "away twice changes nothing");
    assert_eq!(twice.seq, None);
    assert_eq!(changes(&daemon, &client, id).await, [(Presence::Away, PresenceSource::User)]);

    prompt(&daemon, &client, 4, "/dev/pts/1").await;

    assert_eq!(
        changes(&daemon, &client, id).await,
        [(Presence::Away, PresenceSource::User), (Presence::Present, PresenceSource::Input)],
        "the next input ends the away state"
    );
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn presence_set_without_conversation_marks_each_open_one() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let idle = prompt(&daemon, &client, 1, "/dev/pts/1").await.conversation_id;
    daemon.clock().advance(Duration::from_secs(11 * 60));
    let one = prompt(&daemon, &client, 2, "/dev/pts/2").await.conversation_id;
    let two = prompt(&daemon, &client, 3, "/dev/pts/3").await.conversation_id;

    let away: PresenceSetResult = client.call(set(4, None, Presence::Away)).await.unwrap();

    let mut changed = away.changed.clone();
    changed.sort();
    let mut expected = vec![one, two];
    expected.sort();
    assert_eq!(changed, expected, "the conversation that was idle already is left alone");
    for id in [one, two] {
        assert_eq!(changes(&daemon, &client, id).await, [(Presence::Away, PresenceSource::User)]);
    }
    assert_eq!(changes(&daemon, &client, idle).await, []);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn presence_set_of_a_conversation_that_does_not_exist_is_not_found() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let sent = prompt(&daemon, &client, 1, "/dev/pts/1").await;
    let mut missing = sent.conversation_id.to_string();
    missing.replace_range(missing.len() - 4.., "ffff");

    let refused = client
        .call::<PresenceSetResult>(set(2, Some(missing.parse().unwrap()), Presence::Away))
        .await;

    let Err(efr_test_daemon::ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, efr_protocol::ErrorCode::NotFound);
    drop(client);
    daemon.stop().await.unwrap();
}
