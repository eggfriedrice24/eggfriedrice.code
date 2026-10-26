//! `conversations.watch` over a `TestDaemon`: the stream starts with the rows of the
//! recent conversations, then sends a row when a prompt starts a turn and when the turn
//! ends. The phone over the tailnet listener, with a question and its answer, is in
//! `remote.rs`.

use efr_protocol::{
    ConversationId, ConversationStatus, ConversationsWatch, ConversationsWatchItem, Method, Origin,
    RecentConversation,
};
use efr_test_daemon::{ItemStream, Replay, Scenario};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

/// The rows of the snapshot that starts `stream`.
async fn snapshot(stream: &mut ItemStream<ConversationsWatchItem>) -> Vec<RecentConversation> {
    match stream.next().await {
        Some(Ok(ConversationsWatchItem::Snapshot { conversations })) => conversations,
        other => panic!("a watch starts with a snapshot, got {other:?}"),
    }
}

/// Reads the rows of `stream` until one of `conversation_id` has `status`; every row is
/// of that conversation and differs from the row before it.
async fn until_status(
    stream: &mut ItemStream<ConversationsWatchItem>,
    conversation_id: ConversationId,
    status: ConversationStatus,
    before: &mut Option<RecentConversation>,
) -> RecentConversation {
    loop {
        let Some(Ok(ConversationsWatchItem::Changed { conversation })) = stream.next().await else {
            panic!("the watch sends rows until the status {status:?}");
        };
        let conversation = *conversation;
        assert_eq!(conversation.summary.id, conversation_id);
        if let Some(before) = before.as_ref() {
            assert!(shown(before) != shown(&conversation), "a row that shows nothing new");
        }
        *before = Some(conversation.clone());
        if conversation.summary.status == status {
            return conversation;
        }
    }
}

/// What a list shows of `row`, without the time of its last activity.
fn shown(row: &RecentConversation) -> (ConversationStatus, Option<String>, u32) {
    (row.summary.status, row.summary.title.clone(), row.turns)
}

#[tokio::test]
async fn a_watch_starts_with_the_recent_conversations() {
    let replay = Replay::run("single_turn_text").await.unwrap();
    let conversation_id = replay.conversation().unwrap();
    let phone = replay.daemon().connect(replay.daemon().connect_options_from(Origin::Phone));
    let phone = phone.await.unwrap();
    assert_eq!(phone.hello().capabilities.conversations_watch, Some(true));

    let mut stream = phone
        .stream::<ConversationsWatchItem>(Method::ConversationsWatch(ConversationsWatch {
            limit: None,
        }))
        .await
        .unwrap();
    let rows = snapshot(&mut stream).await;
    let [row] = rows.as_slice() else { panic!("one conversation: {rows:?}") };
    assert_eq!(row.summary.id, conversation_id);
    assert_eq!((row.summary.status, row.turns), (ConversationStatus::Idle, 1));
    assert!(row.summary.title.is_some());
    assert!(!row.here, "a client without a terminal holds no row");
    drop((stream, phone));
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn a_new_prompt_sends_its_start_and_its_end() {
    let scenario = Scenario::load("subscribe_resume_after_seq").unwrap();
    let second_prompt = scenario.lines("client_frame")[1];
    let second_start = scenario.event_lines("turn_started")[1];
    let mut replay = Replay::start(scenario).await.unwrap();
    replay.run_to(second_prompt - 1).await.unwrap();
    let conversation_id = replay.conversation().unwrap();

    let phone = replay.daemon().connect(replay.daemon().connect_options_from(Origin::Phone));
    let phone = phone.await.unwrap();
    let mut stream = phone
        .stream::<ConversationsWatchItem>(Method::ConversationsWatch(ConversationsWatch {
            limit: Some(5),
        }))
        .await
        .unwrap();
    let rows = snapshot(&mut stream).await;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].summary.status, rows[0].turns), (ConversationStatus::Idle, 1));

    // NOTE: the turn waits for the model's answer after its start, so its status stays
    // until the replay goes on.
    replay.run_to(second_start).await.unwrap();
    let mut before = Some(rows[0].clone());
    let running =
        until_status(&mut stream, conversation_id, ConversationStatus::Running, &mut before).await;
    assert_eq!(running.turns, 2);
    replay.run_to_end().await.unwrap();
    let idle =
        until_status(&mut stream, conversation_id, ConversationStatus::Idle, &mut before).await;
    assert!(idle.summary.updated_at >= running.summary.updated_at);
    assert!(idle.summary.last_seq > running.summary.last_seq);
    drop((stream, phone));
    replay.stop().await.unwrap();
}
