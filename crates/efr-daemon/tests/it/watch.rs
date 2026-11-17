//! `conversations.watch` over a `TestDaemon`: the stream starts with the rows of the
//! recent conversations, then sends a row when a prompt starts a turn and when the turn
//! ends, and one row when the `attention` of a conversation changes. The phone over the
//! tailnet listener, with a question and its answer, is in `remote.rs`.

use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, Attention, ConversationId,
    ConversationStatus, ConversationsWatch, ConversationsWatchItem, Event, Method, Origin,
    PromptSendResult, QuestionReply, QuestionRespondResult, RecentConversation,
};
use efr_provider::Provider;
use efr_test_daemon::{
    Client, ItemStream, Replay, Scenario, TTY, TestDaemon, answer_question, command_id,
    events_until,
};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel};

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

/// A watch of the recent conversations over `client`, after its snapshot.
async fn watch(client: &Client) -> ItemStream<ConversationsWatchItem> {
    let method = Method::ConversationsWatch(ConversationsWatch { limit: None });
    let mut stream = client.stream::<ConversationsWatchItem>(method).await.unwrap();
    snapshot(&mut stream).await;
    stream
}

/// Reads the rows of `stream` into `seen` until one has `status` and `attention`.
async fn until_row(
    stream: &mut ItemStream<ConversationsWatchItem>,
    seen: &mut Vec<(ConversationStatus, Option<Attention>)>,
    status: ConversationStatus,
    attention: Option<Attention>,
) {
    while seen.last() != Some(&(status, attention)) {
        let Some(Ok(ConversationsWatchItem::Changed { conversation })) = stream.next().await else {
            panic!("the watch sends rows until {status:?} {attention:?}, after {seen:?}");
        };
        seen.push((conversation.summary.status, conversation.summary.attention));
    }
}

/// How many rows of `seen` have `attention`.
fn rows_with(seen: &[(ConversationStatus, Option<Attention>)], attention: Attention) -> usize {
    seen.iter().filter(|(_, row)| *row == Some(attention)).count()
}

#[tokio::test]
async fn a_question_and_a_local_only_approval_each_send_one_row() {
    let question = json!({
        "question": "Which database?",
        "options": [{ "label": "Postgres" }, { "label": "SQLite" }],
    });
    let restart = json!({ "command": "sudo systemctl restart nginx" });
    let model =
        ScriptedModel::reviewed(vec![("ask_user", question), ("shell", restart)], REVIEWER_ALLOWS);
    let daemon =
        TestDaemon::builder().custom_provider(model as Arc<dyn Provider>).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let mut stream = watch(&client).await;
    let mut seen = Vec::new();

    let sent: PromptSendResult =
        client.call(daemon.prompt(1, "set up the store", TTY)).await.unwrap();
    let mut follow = daemon.follow_answering(&client, sent.conversation_id).await.unwrap();
    let events =
        events_until(&mut follow, |event| matches!(event, Event::UserQuestionRequested { .. }))
            .await
            .unwrap();
    let Some(Event::UserQuestionRequested { question_id, .. }) = events.last().map(|e| &e.event)
    else {
        panic!("the model asks: {events:?}");
    };
    use ConversationStatus::{AwaitingApproval, Idle, Running};
    until_row(&mut stream, &mut seen, Running, Some(Attention::Answer)).await;

    let reply = QuestionReply::Option { index: 0 };
    let answer = answer_question(2, sent.conversation_id, *question_id, reply);
    let _: QuestionRespondResult = client.call(answer).await.unwrap();
    let events =
        events_until(&mut follow, |event| matches!(event, Event::ApprovalRequested { .. }))
            .await
            .unwrap();
    let Some(Event::ApprovalRequested { call_id, local_only: true, .. }) =
        events.last().map(|e| &e.event)
    else {
        panic!("a privileged command asks the terminal: {events:?}");
    };
    until_row(&mut stream, &mut seen, AwaitingApproval, Some(Attention::Terminal)).await;

    let deny = Method::ApprovalRespond(ApprovalRespond {
        command_id: command_id(3),
        conversation_id: sent.conversation_id,
        call_id: *call_id,
        decision: ApprovalDecision::Deny,
    });
    let _: ApprovalRespondResult = client.call(deny).await.unwrap();
    until_row(&mut stream, &mut seen, Idle, None).await;

    assert_eq!(rows_with(&seen, Attention::Answer), 1, "{seen:?}");
    assert_eq!(rows_with(&seen, Attention::Terminal), 1, "{seen:?}");
    assert_eq!(rows_with(&seen, Attention::Failed), 0, "{seen:?}");
    drop((stream, follow, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_failed_turn_sends_one_row_and_a_new_watch_shows_it() {
    // NOTE: the empty replay model of a plain test daemon fails each turn at once.
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let mut stream = watch(&client).await;
    let mut seen = Vec::new();

    let sent: PromptSendResult = client.call(daemon.prompt(1, "hi", TTY)).await.unwrap();
    use ConversationStatus::Idle;
    until_row(&mut stream, &mut seen, Idle, Some(Attention::Failed)).await;

    assert_eq!(rows_with(&seen, Attention::Failed), 1, "{seen:?}");
    let method = Method::ConversationsWatch(ConversationsWatch { limit: None });
    let mut again = client.stream::<ConversationsWatchItem>(method).await.unwrap();
    let rows = snapshot(&mut again).await;
    let [row] = rows.as_slice() else { panic!("one conversation: {rows:?}") };
    assert_eq!(row.summary.id, sent.conversation_id);
    assert_eq!((row.summary.status, row.summary.attention), (Idle, Some(Attention::Failed)));
    drop((stream, again, client));
    daemon.stop().await.unwrap();
}
