use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, ApprovalRespondResult, CallId, ClientFrame, ConversationHistoryResult,
    ConversationSnapshot, ConversationStatus, ConversationSubscribe, ConversationSubscribeItem,
    ConversationSummary, ErrorBody, ErrorCode, Event, InputRespond, InputRespondResult, InputWait,
    Method, Origin, RequestId, Seq, TurnId, TurnInterruptResult,
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
    Target { conversation: conversation(), turn: turn(), after: Seq::new(10) }
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
    subscription(conn, after).await.0
}

/// Reads the subscribe request, checks where it starts and returns its params.
async fn subscription(conn: &mut Conn, after: u64) -> (RequestId, ConversationSubscribe) {
    let (id, method) = conn.request().await;
    let Method::ConversationSubscribe(params) = method else {
        panic!("expected a subscribe, got {}", method.name());
    };
    assert_eq!(params.conversation_id, conversation());
    assert_eq!(params.after_seq, Some(Seq::new(after)));
    (id, params)
}

/// The next request, after the cancels of streams the client dropped.
async fn request_after_cancels(conn: &mut Conn) -> (RequestId, Method) {
    loop {
        match conn.recv().await {
            Some(ClientFrame::Cancel { .. }) => {}
            Some(ClientFrame::Request { id, method }) => return (id, method),
            other => panic!("expected a request, got {other:?}"),
        }
    }
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
    run_view(env, ctx, raw_view(), script).await
}

/// Runs `follow` of `view` with `ctx` against the fake daemon, which runs `script`.
async fn run_view<F>(
    env: &TestEnv,
    ctx: &Context,
    mut view: TurnView,
    script: impl FnOnce(Conn, Captured) -> F,
) -> (Result<(), CliError>, String, String)
where
    F: Future<Output = ()>,
{
    let daemon = env.listen();
    let (mut out, captured) = capture();
    let seen = captured.clone();
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
        let updated = Event::AssistantMessageUpdated {
            turn_id: turn(),
            index: 0,
            offset: 0,
            delta: "Hello".to_owned(),
        };
        conn.item(first, &item(11, updated)).await;
        conn.fail(first, ErrorBody::overflow(Seq::new(11))).await;
        let second = subscribed(&mut conn, 11).await;
        // A replayed event is not shown twice.
        let again = Event::AssistantMessageUpdated {
            turn_id: turn(),
            index: 0,
            offset: 0,
            delta: "Hello".to_owned(),
        };
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
async fn a_queued_prompt_shows_and_answers_the_approval_the_running_turn_waits_for() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let mut view = raw_view();
    view.queue();
    let running: TurnId = "0192f0c1-7a00-7000-8000-000000000077".parse().unwrap();
    let parked: CallId = "0192f0c1-7a00-7000-8000-000000000078".parse().unwrap();
    let answered: CallId = "0192f0c1-7a00-7000-8000-000000000079".parse().unwrap();
    let presser = Arc::clone(&keys);
    let (result, _, err) = run_view(&env, &ctx, view, |mut conn, _| async move {
        // The running turn asked before the prompt queued, so the subscription would
        // never replay the question; the newest page of the log has it.
        let (id, method) = conn.request().await;
        let Method::ConversationHistory(params) = method else {
            panic!("expected conversation.history, got {}", method.name());
        };
        assert_eq!(params.conversation_id, conversation());
        let asked = |call_id, summary: &str| Event::ApprovalRequested {
            turn_id: running,
            call_id,
            summary: summary.to_owned(),
            diff_preview: None,
        };
        let page = ConversationHistoryResult {
            events: vec![
                envelope(6, asked(answered, "an earlier question")),
                envelope(
                    7,
                    Event::ApprovalResolved {
                        turn_id: running,
                        call_id: answered,
                        decision: ApprovalDecision::Allow,
                        origin: Origin::Phone,
                    },
                ),
                envelope(8, asked(parked, "write ~/.zshrc")),
            ],
            next_cursor: None,
        };
        conn.reply(id, &page).await;
        let sub = subscribed(&mut conn, 10).await;
        presser.press(b'y').await;
        let (id, method) = conn.request().await;
        let Method::ApprovalRespond(params) = method else {
            panic!("expected approval.respond, got {}", method.name());
        };
        assert_eq!(params.call_id, parked);
        assert_eq!(params.decision, ApprovalDecision::Allow);
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(11) }).await;
        let resolved = Event::ApprovalResolved {
            turn_id: running,
            call_id: parked,
            decision: ApprovalDecision::Allow,
            origin: Origin::Shell,
        };
        conn.item(sub, &item(11, resolved)).await;
        conn.item(sub, &item(12, Event::TurnCompleted { turn_id: running, usage: None })).await;
        let started = Event::TurnStarted {
            turn_id: turn(),
            cwd: std::path::PathBuf::from("/home/u"),
            scope: efr_protocol::Scope::Machine,
        };
        conn.item(sub, &item(13, started)).await;
        // Once the prompt's own turn runs, other turns' approvals are not this view's.
        conn.item(sub, &item(14, asked(answered, "not shown"))).await;
        conn.item(sub, &item(15, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert_eq!(
        err,
        "the running turn needs approval: write ~/.zshrc\nallow? y = yes, n = no\nallowed\n"
    );
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
async fn ctrl_c_interrupts_the_turn_and_keeps_what_arrived() {
    let env = TestEnv::new();
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context { interrupt: interrupt.clone(), ..env.context() };
    let (result, out, err) = run(&env, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        let updated = Event::AssistantMessageUpdated {
            turn_id: turn(),
            index: 0,
            offset: 0,
            delta: "Partial answer".to_owned(),
        };
        conn.item(sub, &item(11, updated)).await;
        // Ctrl+C once the update is on the screen.
        while seen.stdout().is_empty() {
            tokio::task::yield_now().await;
        }
        interrupt.trigger();
        // A closed connection alone would leave the turn running in the daemon.
        let (id, method) = request_after_cancels(&mut conn).await;
        let Method::TurnInterrupt(params) = method else {
            panic!("expected turn.interrupt, got {}", method.name());
        };
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.turn_id, Some(turn()));
        conn.reply(id, &TurnInterruptResult { turn_id: turn(), seq: Seq::new(12) }).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert_eq!(out, "Partial answer\n");
    assert!(err.ends_with("interrupted\n"), "{err:?}");
}

#[tokio::test]
async fn ctrl_c_on_a_queued_prompt_says_it_was_not_interrupted() {
    let env = TestEnv::new();
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context { interrupt: interrupt.clone(), ..env.context() };
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        subscribed(&mut conn, 10).await;
        interrupt.trigger();
        let (id, method) = request_after_cancels(&mut conn).await;
        assert!(matches!(method, Method::TurnInterrupt(_)), "{}", method.name());
        conn.fail(id, ErrorBody::new(ErrorCode::Conflict, "another turn is running")).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert!(
        err.ends_with(
            "not interrupted: the turn is not running; a queued prompt still runs in its turn\n"
        ),
        "{err:?}"
    );
}

fn terminal_view() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(80))
}

fn shell_started(command: &str) -> Event {
    Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: serde_json::json!({ "command": command }),
    }
}

fn shell_output(tail: &str) -> Event {
    Event::ToolCallOutputUpdated {
        turn_id: turn(),
        call_id: call(),
        tail: tail.to_owned(),
        bytes: tail.len() as u64,
    }
}

fn input_changed(input: InputWait) -> Event {
    Event::ToolCallInputChanged { turn_id: turn(), call_id: call(), input }
}

fn shell_completed(exit_code: i32) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: String::new(),
        truncated: false,
        is_error: exit_code != 0,
        exit_code: Some(exit_code),
    }
}

/// Reads the next request, which must be `input.respond`.
async fn input_respond(conn: &mut Conn) -> (RequestId, InputRespond) {
    let (id, method) = conn.request().await;
    let Method::InputRespond(params) = method else {
        panic!("expected input.respond, got {}", method.name());
    };
    (id, params)
}

#[tokio::test]
async fn a_hidden_answer_is_sent_and_never_written_to_the_terminal() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let (sub, params) = subscription(&mut conn, 10).await;
        assert!(params.answers_input, "keys can be read, so a person here can answer");
        conn.item(sub, &item(11, shell_started("sudo pacman -Syu"))).await;
        conn.item(sub, &item(12, shell_output("[sudo] password for egg: "))).await;
        conn.item(sub, &item(13, input_changed(InputWait::Hidden))).await;
        // A typo taken back, an arrow key that means nothing, then Enter.
        presser.type_bytes(b"hunter22\x7f\x1b[D\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.call_id, call());
        assert_eq!(params.text.expose_secret(), "hunter2");
        assert!(params.hidden);
        conn.reply(id, &InputRespondResult {}).await;
        while !seen.stdout().contains("answer sent") {
            tokio::task::yield_now().await;
        }
        conn.item(sub, &item(14, input_changed(InputWait::None))).await;
        conn.item(sub, &item(15, shell_completed(0))).await;
        presser.stopped().await;
        conn.item(sub, &item(16, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert!(keys.discarded(), "the rest of what was typed never reaches the shell");
    assert!(out.contains("it is not shown and the agent does not see it"), "{out}");
    assert!(out.contains("answer sent"), "{out}");
    // Not the answer, not a piece of it, in any frame or note.
    for written in [&out, &err] {
        assert!(!written.contains("hunter"), "{written}");
        assert!(!written.contains("ter2"), "{written}");
    }
}

#[tokio::test]
async fn a_visible_answer_is_echoed_and_sent_as_typed() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo pacman -Syu"))).await;
        conn.item(sub, &item(12, shell_output(":: Proceed with installation? [Y/n] "))).await;
        conn.item(sub, &item(13, input_changed(InputWait::Visible))).await;
        presser.type_bytes("n\u{e4}\x15yes\r".as_bytes()).await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "yes");
        assert!(!params.hidden);
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(14, shell_completed(0))).await;
        presser.stopped().await;
        conn.item(sub, &item(15, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(out.contains("the agent sees it"), "{out}");
    assert!(out.contains("> n\u{e4}"), "the echo grows as the user types: {out}");
    assert!(out.contains("> yes"), "{out}");
    assert!(out.contains("answer sent"), "{out}");
}

#[tokio::test]
async fn an_answer_the_daemon_refuses_is_a_note_and_completion_stops_the_keys() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("ssh host"))).await;
        conn.item(sub, &item(12, shell_output("egg@host's password: "))).await;
        conn.item(sub, &item(13, input_changed(InputWait::Hidden))).await;
        presser.type_bytes(b"secret\r").await;
        let (id, _) = input_respond(&mut conn).await;
        conn.fail(id, ErrorBody::new(ErrorCode::Conflict, "the call does not wait for input"))
            .await;
        while !seen.stdout().contains("nothing was sent") {
            tokio::task::yield_now().await;
        }
        // The command ends without a word about the wait first.
        conn.item(sub, &item(14, shell_completed(255))).await;
        presser.stopped().await;
        conn.item(sub, &item(15, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(out.contains("the command no longer waits for that input; nothing was sent"), "{out}");
    assert!(!out.contains("secret"), "{out}");
    let last_frame = out.rsplit("\x1b[?2026h").next().unwrap_or_default();
    assert!(!last_frame.contains("type the answer"), "the question is gone: {last_frame:?}");
}

#[tokio::test]
async fn without_keys_a_hidden_wait_is_a_note_and_nobody_answers() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let (sub, params) = subscription(&mut conn, 10).await;
        assert!(!params.answers_input, "no terminal on stdin, so nobody here can answer");
        conn.item(sub, &item(11, shell_started("sudo true"))).await;
        conn.item(sub, &item(12, input_changed(InputWait::Hidden))).await;
        conn.item(sub, &item(13, shell_completed(1))).await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(
        err,
        "shell: sudo true\n\
         the command waits for hidden input, such as a password; efr cannot ask for it here\n\
         shell exited with 1\n"
    );
}

#[tokio::test]
async fn keys_stay_quiet_between_two_hidden_asks_of_one_call() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo true"))).await;
        conn.item(sub, &item(12, shell_output("[sudo] password for egg: "))).await;
        conn.item(sub, &item(13, input_changed(InputWait::Hidden))).await;
        presser.type_bytes(b"wrong\r").await;
        let (id, _) = input_respond(&mut conn).await;
        conn.reply(id, &InputRespondResult {}).await;
        while !seen.stdout().contains("answer sent") {
            tokio::task::yield_now().await;
        }
        // sudo checks the password and says it was wrong: no wait for a while.
        conn.item(sub, &item(14, input_changed(InputWait::None))).await;
        conn.item(sub, &item(15, shell_output("Sorry, try again."))).await;
        while !seen.stdout().contains("Sorry, try again.") {
            tokio::task::yield_now().await;
        }
        // Typed while nothing is asked: read and thrown away, never echoed or sent.
        presser.type_bytes(b"hunter2\r").await;
        conn.item(sub, &item(16, shell_output("[sudo] password for egg: "))).await;
        conn.item(sub, &item(17, input_changed(InputWait::Hidden))).await;
        presser.type_bytes(b"right\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "right", "nothing from before the ask");
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(18, input_changed(InputWait::None))).await;
        conn.item(sub, &item(19, shell_completed(0))).await;
        presser.stopped().await;
        conn.item(sub, &item(20, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1, "one reader, so echo never came back in between");
    assert!(keys.discarded());
    for written in [&out, &err] {
        assert!(!written.contains("hunter"), "{written}");
        assert!(!written.contains("right"), "{written}");
    }
}

#[tokio::test]
async fn a_turn_that_ends_while_a_password_is_asked_throws_away_what_was_typed() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, _, _) = run_view(&env, &ctx, terminal_view(), |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo true"))).await;
        conn.item(sub, &item(12, input_changed(InputWait::Hidden))).await;
        presser.type_bytes(b"hunt").await;
        let interrupted = Event::TurnInterrupted { turn_id: turn() };
        conn.item(sub, &item(13, interrupted)).await;
        presser.stopped().await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::TurnInterrupted)), "{result:?}");
    assert!(keys.discarded(), "the half-typed password never reaches the shell");
}
