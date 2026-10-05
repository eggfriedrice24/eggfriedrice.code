//! `conversation.subscribe` over a `TestDaemon`: a resume replays the gap and goes on
//! live without a hole or a repeat; a gap too large starts with a bounded snapshot.

use efr_protocol::{
    ConversationHistory, ConversationHistoryResult, ConversationSubscribe,
    ConversationSubscribeItem, ErrorCode, Event, EventEnvelope, Method, Seq,
};
use efr_test_daemon::{ClientError, Replay, Scenario, TestDaemon, events_until};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

/// The most events a resume replays (the daemon's bound).
const MAX_EVENTS: usize = 128;

fn seqs(events: &[EventEnvelope]) -> Vec<u64> {
    events.iter().map(|envelope| envelope.seq.get()).collect()
}

#[tokio::test]
async fn subscribe_resume_after_seq() {
    let scenario = Scenario::load("subscribe_resume_after_seq").unwrap();
    let second_prompt = scenario.lines("client_frame")[1];
    let mut replay = Replay::start(scenario).await.unwrap();
    replay.run_to(second_prompt - 1).await.unwrap();

    // A client that saw the first turn up to its prompt comes back.
    let first_turn = replay.events().await.unwrap();
    let after = first_turn[1].seq;
    let watcher = replay.daemon().client().await.unwrap();
    let conversation_id = replay.conversation().unwrap();
    let mut resumed = watcher
        .stream::<ConversationSubscribeItem>(Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id,
            after_seq: Some(after),
            answers_input: false,
        }))
        .await
        .unwrap();
    replay.run_to_end().await.unwrap();

    let mut completed = 0;
    let seen = events_until(&mut resumed, |event| {
        if matches!(event, Event::TurnCompleted { .. }) {
            completed += 1;
        }
        completed == 2
    })
    .await;
    let seen = seen.unwrap();
    let log = replay.events().await.unwrap();
    let expected: Vec<EventEnvelope> =
        log.into_iter().filter(|envelope| envelope.seq > after).collect();
    assert_eq!(seqs(&seen), seqs(&expected), "the gap, then live, each event once");
    assert_eq!(seen, expected);
    drop((resumed, watcher));
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn subscribe_gap_too_large_snapshot() {
    let replay = Replay::run("subscribe_gap_too_large_snapshot").await.unwrap();
    let log = replay.events().await.unwrap();
    assert!(log.len() > MAX_EVENTS, "the scenario makes a log longer than a resume replays");
    let conversation_id = replay.conversation().unwrap();

    for after_seq in [Some(Seq::ZERO), None] {
        let mut stream = replay
            .client()
            .stream::<ConversationSubscribeItem>(Method::ConversationSubscribe(
                ConversationSubscribe { conversation_id, after_seq, answers_input: false },
            ))
            .await
            .unwrap();
        let first = stream.next().await.unwrap().unwrap();
        let ConversationSubscribeItem::Snapshot(snapshot) = first else {
            panic!("a gap of {} events must start with a snapshot, got {first:?}", log.len());
        };
        assert_eq!(snapshot.conversation.id, conversation_id);
        assert_eq!(snapshot.hwm, log.last().unwrap().seq);
        assert_eq!(snapshot.events.len(), MAX_EVENTS);
        assert_eq!(snapshot.events.as_slice(), &log[log.len() - MAX_EVENTS..], "the newest");

        // The cursor pages back to the events the snapshot left out.
        let older: ConversationHistoryResult = replay
            .client()
            .call(Method::ConversationHistory(ConversationHistory {
                conversation_id,
                cursor: snapshot.history_cursor.clone(),
                limit: Some(500),
            }))
            .await
            .unwrap();
        assert_eq!(older.events.as_slice(), &log[..log.len() - MAX_EVENTS]);
        assert_eq!(older.next_cursor, None);
    }
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn a_small_gap_is_replayed_event_by_event() {
    let replay = Replay::run("single_turn_text").await.unwrap();
    let log = replay.events().await.unwrap();
    let conversation_id = replay.conversation().unwrap();

    let mut stream = replay
        .client()
        .stream::<ConversationSubscribeItem>(Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id,
            after_seq: Some(log[2].seq),
            answers_input: false,
        }))
        .await
        .unwrap();
    let mut replayed = Vec::new();
    for _ in 3..log.len() {
        match stream.next().await.unwrap().unwrap() {
            ConversationSubscribeItem::Event(envelope) => replayed.push(envelope),
            other => panic!("a small gap is replayed, not summarised: {other:?}"),
        }
    }
    assert_eq!(replayed.as_slice(), &log[3..]);
    drop(stream);
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn subscribing_to_a_conversation_that_does_not_exist_is_not_found() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let conversation_id = "0192f0c1-7a00-7000-8000-00000000dead".parse().unwrap();

    let mut stream = daemon.follow(&client, conversation_id).await.unwrap();
    let ended = stream.next().await.unwrap();

    let Err(ClientError::Server { body }) = ended else { panic!("{ended:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    drop((stream, client));
    daemon.stop().await.unwrap();
}
