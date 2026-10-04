use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, ApprovalRespondResult, ClientFrame, ConversationSnapshot, ConversationStatus,
    ConversationSubscribeItem, ConversationSummary, ErrorBody, ErrorCode, Event, Method, Origin,
    RequestId, Seq,
};
use efr_render::RenderOptions;
use pretty_assertions::assert_eq;

use super::{Target, TurnView, follow};
use crate::context::Context;
use crate::error::CliError;
use crate::testing::{
    Captured, Conn, ScriptedKeys, TestEnv, TestInterrupt, call, capture, conversation, envelope,
    item, now, turn,
};

fn target() -> Target {
    Target { conversation: conversation(), after: Seq::new(10) }
}

fn raw_view() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(80).with_terminal(false))
}

fn completed(text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id: turn(), index: 0, text: text.to_owned() }
}

fn turn_completed() -> Event {
    Event::TurnCompleted { turn_id: turn(), usage: None }
}

/// Reads the subscribe request and checks where it starts.
async fn subscribed(conn: &mut Conn, after: u64) -> RequestId {
    let (id, method) = conn.request().await;
    let Method::ConversationSubscribe(params) = method else {
        panic!("expected a subscribe, got {}", method.name());
    };
    assert_eq!(params.conversation_id, conversation());
    assert_eq!(params.after_seq, Some(Seq::new(after)));
    id
}

/// Runs `follow` with `ctx` against the fake daemon, which runs `script`.
async fn run<F>(
    env: &TestEnv,
    ctx: &Context,
    script: impl FnOnce(Conn, Captured) -> F,
) -> (Result<(), CliError>, String, String)
where
    F: Future<Output = ()>,
{
    let daemon = env.listen();
    let (mut out, captured) = capture();
    let seen = captured.clone();
    let mut view = raw_view();
    let client = async {
        let client = ctx.connect(Origin::Cli, None).await.unwrap();
        follow(ctx, &client, &mut out, &mut view, target()).await
    };
    let daemon = async { script(daemon.accept().await, seen).await };
    let (result, ()) = tokio::join!(client, daemon);
    (result, captured.stdout(), captured.stderr())
}

#[tokio::test]
async fn a_turn_is_followed_from_after_its_prompt_to_its_end() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, out, _) = run(&env, &ctx, |mut conn, _| async move {
        let id = subscribed(&mut conn, 10).await;
        conn.item(id, &item(11, completed("Done."))).await;
        conn.item(id, &item(12, turn_completed())).await;
        // The finished stream is cancelled when the command drops it.
        let rest = conn.until_closed().await;
        assert_eq!(rest, [ClientFrame::Cancel { id }]);
    })
    .await;
    result.unwrap();
    assert_eq!(out, "Done.\n");
}

#[tokio::test]
async fn an_overflowed_subscription_resumes_after_the_last_event_shown() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, out, _) = run(&env, &ctx, |mut conn, _| async move {
        let first = subscribed(&mut conn, 10).await;
        let updated =
            Event::AssistantMessageUpdated { turn_id: turn(), index: 0, text: "Hello".to_owned() };
        conn.item(first, &item(11, updated)).await;
        conn.fail(first, ErrorBody::overflow(Seq::new(11))).await;
        let second = subscribed(&mut conn, 11).await;
        // A replayed event is not shown twice.
        let again =
            Event::AssistantMessageUpdated { turn_id: turn(), index: 0, text: "Hello".to_owned() };
        conn.item(second, &item(11, again)).await;
        conn.item(second, &item(12, completed("Hello."))).await;
        conn.item(second, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(out, "Hello.\n");
}

#[tokio::test]
async fn a_snapshot_is_shown_and_counts_up_to_its_high_water_mark() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, out, _) = run(&env, &ctx, |mut conn, _| async move {
        let first = subscribed(&mut conn, 10).await;
        let snapshot = ConversationSnapshot {
            conversation: ConversationSummary {
                id: conversation(),
                title: None,
                status: ConversationStatus::Running,
                created_at: now(),
                updated_at: now(),
                last_seq: Seq::new(40),
                cwd: None,
                scope: None,
                tty: None,
            },
            events: vec![envelope(39, completed("From the snapshot."))],
            history_cursor: None,
            hwm: Seq::new(40),
        };
        conn.item(first, &ConversationSubscribeItem::Snapshot(snapshot)).await;
        conn.fail(first, ErrorBody::overflow(Seq::new(40))).await;
        let second = subscribed(&mut conn, 40).await;
        conn.item(second, &item(41, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(out, "From the snapshot.\n");
}

#[tokio::test]
async fn an_unknown_item_is_skipped() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, out, _) = run(&env, &ctx, |mut conn, _| async move {
        let id = subscribed(&mut conn, 10).await;
        conn.item(id, &serde_json::json!({ "kind": "hologram", "seq": 11 })).await;
        conn.item(id, &item(12, completed("Still here."))).await;
        conn.item(id, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(out, "Still here.\n");
}

#[tokio::test]
async fn a_subscription_that_ends_before_the_turn_is_an_error() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, _, _) = run(&env, &ctx, |mut conn, _| async move {
        let id = subscribed(&mut conn, 10).await;
        conn.end(id).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::SubscriptionEnded)), "{result:?}");
}

#[tokio::test]
async fn a_subscription_that_keeps_falling_behind_gives_up() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, _, _) = run(&env, &ctx, |mut conn, _| async move {
        for _ in 0..9 {
            let id = subscribed(&mut conn, 10).await;
            conn.fail(id, ErrorBody::overflow(Seq::new(10))).await;
        }
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::FellBehind { times: 9 })), "{result:?}");
}

#[tokio::test]
async fn a_failed_turn_is_an_error_with_the_daemons_message() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, _, _) = run(&env, &ctx, |mut conn, _| async move {
        let id = subscribed(&mut conn, 10).await;
        let error = ErrorBody::new(ErrorCode::Internal, "the provider is down");
        conn.item(id, &item(11, Event::TurnFailed { turn_id: turn(), error })).await;
        conn.until_closed().await;
    })
    .await;
    let Err(CliError::TurnFailed { body }) = result else { panic!("{result:?}") };
    assert_eq!(body.message, "the provider is down");
}

#[tokio::test]
async fn a_key_answers_the_approval() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        let request = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "run rm -rf build".to_owned(),
            diff_preview: None,
        };
        conn.item(sub, &item(11, request)).await;
        // A key that answers nothing is ignored; `n` denies.
        presser.press(b'x').await;
        presser.press(b'n').await;
        let (id, method) = conn.request().await;
        let Method::ApprovalRespond(params) = method else {
            panic!("expected approval.respond, got {}", method.name());
        };
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.call_id, call());
        assert_eq!(params.decision, ApprovalDecision::Deny);
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(12) }).await;
        let resolved = Event::ApprovalResolved {
            turn_id: turn(),
            call_id: call(),
            decision: ApprovalDecision::Deny,
            origin: Origin::Shell,
        };
        conn.item(sub, &item(12, resolved)).await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert_eq!(err, "approval needed: run rm -rf build\nallow? y = yes, n = no\ndenied\n");
}

#[tokio::test]
async fn an_answer_the_daemon_no_longer_takes_is_a_note() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        let request = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "edit /etc/hosts".to_owned(),
            diff_preview: None,
        };
        conn.item(sub, &item(11, request)).await;
        keys.press(b'y').await;
        let (id, _) = conn.request().await;
        conn.fail(id, ErrorBody::new(ErrorCode::NotFound, "no pending approval")).await;
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(err.ends_with("allowed\nthe answer was not taken: no pending approval\n"), "{err}");
}

#[tokio::test]
async fn ctrl_c_closes_the_connection_and_keeps_what_arrived() {
    let env = TestEnv::new();
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context { interrupt: interrupt.clone(), ..env.context() };
    let (result, out, err) = run(&env, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        let updated = Event::AssistantMessageUpdated {
            turn_id: turn(),
            index: 0,
            text: "Partial answer".to_owned(),
        };
        conn.item(sub, &item(11, updated)).await;
        // Ctrl+C once the update is on the screen.
        while seen.stdout().is_empty() {
            tokio::task::yield_now().await;
        }
        interrupt.trigger();
        // The daemon sees the connection close, which is what cancels the turn.
        let rest = conn.until_closed().await;
        assert!(rest.iter().all(|frame| matches!(frame, ClientFrame::Cancel { .. })), "{rest:?}");
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert_eq!(out, "Partial answer\n");
    assert!(err.ends_with("interrupted\n"), "{err:?}");
}
