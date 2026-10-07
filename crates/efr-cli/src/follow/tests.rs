use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, ApprovalRespondResult, CallId, ClientFrame, ConversationHistoryResult,
    ConversationSnapshot, ConversationStatus, ConversationSubscribe, ConversationSubscribeItem,
    ConversationSummary, Draft, DraftPart, ErrorBody, ErrorCode, Event, InputRespond,
    InputRespondResult, InputWait, Method, Origin, QuestionId, RequestId,
    SandboxSurfaceRespondResult, Seq, SurfaceChange, TurnId, TurnInterruptResult,
};
use efr_render::RenderOptions;
use efr_test_support::{TestClock, Wait};
use pretty_assertions::assert_eq;

use super::{AnswerKind, Ask, Asking, Look, Seed, Target, TurnView, follow, since_then, take_over};
use crate::answer::AnswerLine;
use crate::context::Context;
use crate::error::CliError;
use crate::keys::KeyReader;
use crate::progress;
use crate::terminal::Size;
use crate::testing::{
    Captured, Conn, GateClock, ResizableScreen, ScriptedKeys, TestEnv, TestInterrupt, TestQuit,
    TestResize, TestResume, TestTerminate, call, capture, conversation, envelope, item, now,
    readable, turn,
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
    assert!(params.drafts, "the view merges drafts");
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
            interactive: false,
            exit: None,
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
    assert_eq!(
        err,
        "? allow this call\n\u{2502} run rm -rf build\n\u{2502} y allow \u{b7} n deny\n\u{2717} denied\n"
    );
}

#[tokio::test]
async fn a_key_answers_the_quarantine_question_with_its_own_id() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let question: QuestionId = "019a9b1c-3d00-7a10-8b20-0000000000d1".parse().unwrap();
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        let request = Event::SurfaceQuestionRequested {
            turn_id: turn(),
            call_id: call(),
            question_id: question,
            changes: vec![SurfaceChange {
                path: PathBuf::from("/p/app/.git/commondir"),
                rule: "commondir_in_main_git_dir".to_owned(),
                key: Some("core.fsmonitor".to_owned()),
                quarantined: true,
            }],
        };
        conn.item(sub, &item(11, request)).await;
        // A key that answers nothing is ignored; `y` keeps the change.
        presser.press(b'x').await;
        presser.press(b'y').await;
        let (id, method) = conn.request().await;
        let Method::SandboxSurfaceRespond(params) = method else {
            panic!("expected sandbox.surface_respond, got {}", method.name());
        };
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.question_id, question);
        assert!(params.keep);
        conn.reply(id, &SandboxSurfaceRespondResult { seq: Seq::new(12) }).await;
        let answered = Event::SurfaceQuestionAnswered {
            turn_id: turn(),
            question_id: question,
            keep: true,
            origin: Some(Origin::Shell),
        };
        conn.item(sub, &item(12, answered)).await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert_eq!(
        err,
        "\
? keep the git settings that the last command changed
\u{2502} /p/app/.git/commondir (core.fsmonitor); moved to quarantine
\u{2502} y keep \u{b7} n leave in quarantine
\u{2713} kept
"
    );
}

#[tokio::test]
async fn a_quarantine_answer_the_daemon_refuses_is_a_note() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let question: QuestionId = "019a9b1c-3d00-7a10-8b20-0000000000d1".parse().unwrap();
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        let request = Event::SurfaceQuestionRequested {
            turn_id: turn(),
            call_id: call(),
            question_id: question,
            changes: Vec::new(),
        };
        conn.item(sub, &item(11, request)).await;
        presser.press(b'n').await;
        let (id, _) = conn.request().await;
        conn.fail(id, ErrorBody::new(ErrorCode::NotFound, "the question is not pending")).await;
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(
        err.ends_with(
            "\u{2717} left in quarantine\n\nthe answer was not taken: the question is not pending\n"
        ),
        "{err}"
    );
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
            interactive: false,
            exit: None,
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
            cwd: PathBuf::from("/home/u"),
            scope: efr_protocol::Scope::Machine,
            settings: None,
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
        "? the running turn asks: allow this call\n\u{2502} write ~/.zshrc\n\
         \u{2502} y allow \u{b7} n deny\n\u{2713} allowed\n"
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
            interactive: false,
            exit: None,
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
    assert!(
        err.ends_with("\u{2713} allowed\n\nthe answer was not taken: no pending approval\n"),
        "{err}"
    );
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
        shows_on(&seen, Stream::Stdout, |text| !text.is_empty()).await;
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
        manual_input: true,
        launch: None,
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
    Event::ToolCallInputChanged { turn_id: turn(), call_id: call(), input, looks_secret: false }
}

fn shell_completed(exit_code: i32) -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: String::new(),
        truncated: false,
        is_error: exit_code != 0,
        exit_code: Some(exit_code),
        sandbox: None,
        refusal: None,
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
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, _| async move {
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
    assert!(out.contains("the agent sees it only if the program prints it"), "{out}");
    // A password gets no note when it is sent.
    assert!(!out.contains("answer sent"), "{out}");
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
    assert!(out.contains("the agent sees it if the program shows it"), "{out}");
    assert!(out.contains("> n\u{e4}"), "the echo grows as the user types: {out}");
    assert!(out.contains("> yes"), "{out}");
    assert!(out.contains("answer sent"), "{out}");
}

/// Asks to approve the shell call as one that may wait for input, and allows it with
/// `y`, the first key typed.
async fn allow_interactive(conn: &mut Conn, sub: RequestId, presser: &ScriptedKeys) {
    let request = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "shell: sudo pacman -Syu".to_owned(),
        diff_preview: None,
        interactive: true,
        exit: None,
    };
    conn.item(sub, &item(12, request)).await;
    presser.press(b'y').await;
    let (id, method) = conn.request().await;
    let Method::ApprovalRespond(params) = method else {
        panic!("expected approval.respond, got {}", method.name());
    };
    assert_eq!(params.decision, ApprovalDecision::Allow);
    conn.reply(id, &ApprovalRespondResult { seq: Seq::new(13) }).await;
    let resolved = Event::ApprovalResolved {
        turn_id: turn(),
        call_id: call(),
        decision: ApprovalDecision::Allow,
        origin: Origin::Shell,
    };
    conn.item(sub, &item(13, resolved)).await;
}

/// The end of the shell call and of the turn, once the keys stopped.
async fn finish_shell(conn: &mut Conn, sub: RequestId, presser: &ScriptedKeys, seq: u64) {
    conn.item(sub, &item(seq, shell_completed(0))).await;
    presser.stopped().await;
    conn.item(sub, &item(seq + 1, turn_completed())).await;
    conn.until_closed().await;
}

#[tokio::test]
async fn keys_typed_ahead_for_an_allowed_interactive_call_start_its_visible_answer() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo pacman -Syu"))).await;
        allow_interactive(&mut conn, sub, &presser).await;
        // Typed while the command still works, with an Enter that must not send it.
        presser.type_bytes(b"Y\r").await;
        conn.item(sub, &item(14, shell_output(":: Proceed with installation? [Y/n] "))).await;
        conn.item(sub, &item(15, input_changed(InputWait::Visible))).await;
        shows(&seen, "> Y").await;
        presser.type_bytes(b"es\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "Yes", "one answer, sent by the last Enter");
        assert!(!params.hidden);
        conn.reply(id, &InputRespondResult {}).await;
        shows(&seen, "answer sent").await;
        // The wait ends and the keys are kept for the call again.
        conn.item(sub, &item(16, input_changed(InputWait::None))).await;
        finish_shell(&mut conn, sub, &presser, 17).await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1, "the reader that read the y read everything after it");
    assert!(keys.discarded(), "what is still unread at the call's end never reaches the shell");
    assert!(out.contains("answer sent"), "{out}");
}

#[tokio::test]
async fn keys_typed_ahead_never_join_a_hidden_answer_and_never_show() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo pacman -Syu"))).await;
        allow_interactive(&mut conn, sub, &presser).await;
        presser.type_bytes(b"ahead\r").await;
        conn.item(sub, &item(14, shell_output("[sudo] password for egg: "))).await;
        conn.item(sub, &item(15, input_changed(InputWait::Hidden))).await;
        shows(&seen, "it is not shown").await;
        presser.type_bytes(b"pw\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "pw");
        assert!(params.hidden);
        conn.reply(id, &InputRespondResult {}).await;
        finish_shell(&mut conn, sub, &presser, 16).await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    for written in [&out, &err] {
        assert!(!written.contains("ahead"), "{written}");
    }
}

#[tokio::test]
async fn keys_typed_ahead_start_a_secret_looking_answer_without_showing() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo -u build passwd"))).await;
        allow_interactive(&mut conn, sub, &presser).await;
        presser.type_bytes(b"hunter2").await;
        conn.item(sub, &item(14, shell_output("Current password: "))).await;
        let secret = Event::ToolCallInputChanged {
            turn_id: turn(),
            call_id: call(),
            input: InputWait::Visible,
            looks_secret: true,
        };
        conn.item(sub, &item(15, secret)).await;
        shows(&seen, "your typing is not shown here").await;
        shows(&seen, "the answer starts with 7 characters typed ahead; Ctrl+U clears them").await;
        presser.press(b'\r').await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "hunter2");
        assert!(!params.hidden, "it goes as the visible answer that the wait asked for");
        conn.reply(id, &InputRespondResult {}).await;
        finish_shell(&mut conn, sub, &presser, 16).await;
    })
    .await;
    result.unwrap();
    for written in [&out, &err] {
        assert!(!written.contains("hunter") && !written.contains("ter2"), "{written}");
    }
}

#[tokio::test]
async fn keys_typed_during_a_call_allowed_without_input_stay_for_the_shell() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, _, _) = run_view(&env, &ctx, terminal_view(), |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("make clean"))).await;
        let request = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "shell: make clean".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
        };
        conn.item(sub, &item(12, request)).await;
        presser.press(b'y').await;
        let (id, _) = conn.request().await;
        // The reader stops at once and leaves what is unread to the user's shell.
        presser.stopped().await;
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(13) }).await;
        finish_shell(&mut conn, sub, &presser, 13).await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert!(!keys.discarded());
}

#[tokio::test]
async fn without_a_terminal_on_stdout_a_visible_answer_is_echoed_on_stderr_and_a_hidden_one_is_not()
{
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, raw_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo pacman -Syu"))).await;
        conn.item(sub, &item(12, shell_output("[sudo] password for egg: "))).await;
        conn.item(sub, &item(13, input_changed(InputWait::Hidden))).await;
        presser.type_bytes(b"hunter2\r").await;
        let (id, _) = input_respond(&mut conn).await;
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(14, shell_output(":: Proceed with installation? [Y/n] "))).await;
        conn.item(sub, &item(15, input_changed(InputWait::Visible))).await;
        shows_on(&seen, Stream::Stderr, |text| text.ends_with("> ")).await;
        presser.type_bytes(b"yo\x7fes\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "yes");
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(16, shell_completed(0))).await;
        presser.stopped().await;
        conn.item(sub, &item(17, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(out, "");
    assert!(err.contains("> yo\u{8} \u{8}es\n\nanswer sent\n"), "{err:?}");
    assert_eq!(err.matches("answer sent").count(), 1, "only the visible answer: {err:?}");
    assert!(!err.contains("hunter") && !err.contains("ter2"), "{err:?}");
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
        shows(&seen, "nothing was sent").await;
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
        "\u{b7} $ sudo true\n\n\
         the command waits for hidden input, such as a password; efr cannot ask for it here\n  \
         \u{2717} exit 1\n"
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
        // sudo checks the password and says it was wrong: no wait for a while.
        conn.item(sub, &item(14, input_changed(InputWait::None))).await;
        conn.item(sub, &item(15, shell_output("Sorry, try again."))).await;
        shows(&seen, "Sorry, try again.").await;
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

#[tokio::test]
async fn a_queued_prompt_asks_for_the_password_the_running_turn_waits_for() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let mut view = terminal_view();
    view.queue();
    let running: TurnId = "0192f0c1-7a00-7000-8000-000000000077".parse().unwrap();
    let earlier: CallId = "0192f0c1-7a00-7000-8000-000000000078".parse().unwrap();
    let sudo: CallId = "0192f0c1-7a00-7000-8000-000000000079".parse().unwrap();
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, view, |mut conn, _| async move {
        // The running turn's command asked before the prompt queued, so only the
        // newest page of the log has the wait.
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationHistory(_)), "{}", method.name());
        let output = |call_id, tail: &str| Event::ToolCallOutputUpdated {
            turn_id: running,
            call_id,
            tail: tail.to_owned(),
            bytes: 1,
        };
        let wait = |call_id, input| Event::ToolCallInputChanged {
            turn_id: running,
            call_id,
            input,
            looks_secret: false,
        };
        let page = ConversationHistoryResult {
            events: vec![
                envelope(3, output(earlier, "Proceed? [Y/n] ")),
                envelope(4, wait(earlier, InputWait::Visible)),
                envelope(
                    5,
                    Event::ToolCallCompleted {
                        turn_id: running,
                        call_id: earlier,
                        output: String::new(),
                        truncated: false,
                        is_error: false,
                        exit_code: Some(0),
                        sandbox: None,
                        refusal: None,
                    },
                ),
                envelope(6, output(sudo, "[sudo] password for egg: ")),
                envelope(7, wait(sudo, InputWait::Hidden)),
            ],
            next_cursor: None,
        };
        conn.reply(id, &page).await;
        let (sub, params) = subscription(&mut conn, 10).await;
        assert!(params.answers_input);
        presser.type_bytes(b"hunter2\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.call_id, sudo, "the call that waits, not the finished one");
        assert!(params.hidden);
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(11, wait(sudo, InputWait::None))).await;
        let completed = Event::ToolCallCompleted {
            turn_id: running,
            call_id: sudo,
            output: String::new(),
            truncated: false,
            is_error: false,
            exit_code: Some(0),
            sandbox: None,
            refusal: None,
        };
        conn.item(sub, &item(12, completed)).await;
        presser.stopped().await;
        conn.item(sub, &item(13, Event::TurnCompleted { turn_id: running, usage: None })).await;
        let started = Event::TurnStarted {
            turn_id: turn(),
            cwd: PathBuf::from("/home/u"),
            scope: efr_protocol::Scope::Machine,
            settings: None,
        };
        conn.item(sub, &item(14, started)).await;
        conn.item(sub, &item(15, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert!(keys.discarded());
    assert!(out.contains("[sudo] password for egg:"), "{out}");
    assert!(!out.contains("Proceed?"), "a finished call is not shown: {out}");
    for written in [&out, &err] {
        assert!(!written.contains("hunter"), "{written}");
    }
}

#[tokio::test]
async fn a_secret_looking_visible_answer_is_not_shown_and_goes_as_a_visible_one() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo -u build passwd"))).await;
        conn.item(sub, &item(12, shell_output("Current password: "))).await;
        let wait = Event::ToolCallInputChanged {
            turn_id: turn(),
            call_id: call(),
            input: InputWait::Visible,
            looks_secret: true,
        };
        conn.item(sub, &item(13, wait)).await;
        presser.type_bytes(b"hunter2\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "hunter2");
        assert!(!params.hidden, "the daemon reported a visible wait");
        assert!(!params.manual);
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(14, input_changed(InputWait::None))).await;
        conn.item(sub, &item(15, shell_completed(0))).await;
        presser.stopped().await;
        conn.item(sub, &item(16, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(out.contains("your typing is not shown here"), "{out}");
    assert!(keys.discarded(), "the rest of what was typed never reaches the shell");
    for written in [&out, &err] {
        assert!(!written.contains("hunter"), "{written}");
        assert!(!written.contains("ter2"), "{written}");
    }
}

const HINT: &str = "no output for 10 s; press Ctrl+\\ to type an input for the command";

/// Waits until stdout holds `text`.
async fn shows(seen: &Captured, text: &str) {
    shows_on(seen, Stream::Stdout, |shown| shown.contains(text)).await;
}

/// Where a view writes.
#[derive(Debug, Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

/// Waits until what the view wrote to `stream` passes `test`.
async fn shows_on(seen: &Captured, stream: Stream, test: impl Fn(&str) -> bool) {
    Wait::new(&format!("the view's {stream:?}"))
        .until(|| match stream {
            Stream::Stdout => test(&seen.stdout()),
            Stream::Stderr => test(&seen.stderr()),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn ctrl_backslash_after_a_silence_types_a_manual_answer() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let quit = Arc::new(TestQuit::default());
    let clock = Arc::new(GateClock::default());
    let ctx =
        Context { keys: keys.clone(), quit: quit.clone(), clock: clock.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (pressing, timing) = (Arc::clone(&quit), Arc::clone(&clock));
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("./deploy"))).await;
        conn.item(sub, &item(12, shell_output("deploying\n"))).await;
        timing.until_slept(super::SILENCE).await;
        assert_eq!(pressing.armed(), 0, "the key is not taken before the hint");
        timing.open();
        shows(&seen, HINT).await;
        pressing.until_armed(1).await;
        assert_eq!(presser.starts(), 0, "the hint reads no key");
        pressing.trigger();
        presser.type_bytes(b"yes\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.call_id, call());
        assert_eq!(params.text.expose_secret(), "yes");
        assert!(params.manual);
        assert!(!params.hidden);
        conn.reply(id, &InputRespondResult {}).await;
        shows(&seen, "answer sent").await;
        // One manual answer, then the keys go back to the shell.
        presser.stopped().await;
        conn.item(sub, &item(13, shell_completed(0))).await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert!(!out.contains("yes"), "the manual answer is not shown as it is typed: {out}");
    assert_eq!(quit.armed(), 0, "the key is let go when the turn ends");
}

#[tokio::test]
async fn keys_typed_after_a_manual_answer_of_an_allowed_interactive_call_never_start_a_line() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let quit = Arc::new(TestQuit::default());
    let clock = Arc::new(GateClock::default());
    let ctx =
        Context { keys: keys.clone(), quit: quit.clone(), clock: clock.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (pressing, timing) = (Arc::clone(&quit), Arc::clone(&clock));
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("./deploy"))).await;
        allow_interactive(&mut conn, sub, &presser).await;
        // The silence starts again once the approval is answered, so its sleep may
        // begin after any one opening of the gate.
        Wait::new("the hint")
            .until(|| {
                timing.open();
                seen.stdout().contains(HINT)
            })
            .await
            .unwrap();
        pressing.until_armed(1).await;
        pressing.trigger();
        // The reader already runs, so the line must be open before the keys come.
        shows(&seen, "or Ctrl+\\ to cancel").await;
        presser.type_bytes(b"hunter2\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert!(params.manual);
        assert_eq!(params.text.expose_secret(), "hunter2");
        conn.reply(id, &InputRespondResult {}).await;
        shows(&seen, "answer sent").await;
        // A second copy typed ahead for a retype prompt that efr cannot see.
        presser.type_bytes(b"hunter2").await;
        conn.item(sub, &item(14, shell_output("Proceed? [y/n] "))).await;
        conn.item(sub, &item(15, input_changed(InputWait::Visible))).await;
        shows(&seen, "the agent sees it if the program shows it").await;
        presser.type_bytes(b"y\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "y", "nothing typed before the question");
        conn.reply(id, &InputRespondResult {}).await;
        shows(&seen, "answer sent").await;
        conn.item(sub, &item(16, shell_completed(0))).await;
        presser.stopped().await;
        conn.item(sub, &item(17, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    for written in [&out, &err] {
        assert!(!written.contains("hunter") && !written.contains("ter2"), "{written}");
    }
}

/// Waits until `keys` has started `count` readers.
async fn started(keys: &ScriptedKeys, count: usize) {
    Wait::new(&format!("{count} key readers")).until(|| keys.starts() == count).await.unwrap();
}

#[tokio::test]
async fn ctrl_backslash_again_closes_the_manual_line_unsent_and_efr_goes_on() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let quit = Arc::new(TestQuit::default());
    let clock = Arc::new(GateClock::default());
    let ctx =
        Context { keys: keys.clone(), quit: quit.clone(), clock: clock.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (pressing, timing) = (Arc::clone(&quit), Arc::clone(&clock));
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("./deploy"))).await;
        timing.until_slept(super::SILENCE).await;
        timing.open();
        shows(&seen, HINT).await;
        pressing.until_armed(1).await;
        pressing.trigger();
        started(&presser, 1).await;
        // The key is still taken while the line reads keys: a second press must not end
        // efr with the terminal left in the key reader's modes.
        pressing.until_taken(1).await;
        pressing.until_armed(1).await;
        presser.type_bytes(b"ye").await;
        pressing.trigger();
        presser.stopped().await;
        assert!(presser.discarded(), "what was typed is thrown away");
        shows(&seen, "the input was not sent").await;
        pressing.until_taken(2).await;
        pressing.until_armed(1).await;
        // The line is offered again, and a third press opens it anew.
        pressing.trigger();
        started(&presser, 2).await;
        presser.type_bytes(b"yes\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "yes", "the closed line sent nothing");
        conn.reply(id, &InputRespondResult {}).await;
        presser.stopped().await;
        conn.item(sub, &item(12, shell_completed(0))).await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!out.contains("ye"), "{out}");
    assert_eq!(quit.armed(), 0);
}

#[tokio::test]
async fn ctrl_backslash_during_an_approval_is_taken_and_does_nothing() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let quit = Arc::new(TestQuit::default());
    let ctx = Context { keys: keys.clone(), quit: quit.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let pressing = Arc::clone(&quit);
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        let request = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "run rm -rf build".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
        };
        conn.item(sub, &item(11, request)).await;
        started(&presser, 1).await;
        pressing.until_armed(1).await;
        pressing.trigger();
        pressing.until_taken(1).await;
        pressing.until_armed(1).await;
        // The approval still reads its key.
        presser.press(b'y').await;
        let (id, method) = conn.request().await;
        let Method::ApprovalRespond(params) = method else {
            panic!("expected approval.respond, got {}", method.name());
        };
        assert_eq!(params.decision, ApprovalDecision::Allow);
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(12) }).await;
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    assert!(err.ends_with("allowed\n"), "{err}");
    assert_eq!(quit.armed(), 0, "the key is let go when the keys stop");
}

#[tokio::test]
async fn a_silent_call_without_ctrl_backslash_reads_no_keys_and_lets_go_of_the_key() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let quit = Arc::new(TestQuit::default());
    let clock = Arc::new(GateClock::default());
    let ctx =
        Context { keys: keys.clone(), quit: quit.clone(), clock: clock.clone(), ..env.context() };
    let (pressing, timing) = (Arc::clone(&quit), Arc::clone(&clock));
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("./deploy"))).await;
        timing.until_slept(super::SILENCE).await;
        timing.open();
        shows(&seen, HINT).await;
        pressing.until_armed(1).await;
        // The command prints again: the hint goes and the key is let go.
        conn.item(sub, &item(12, shell_output("done\n"))).await;
        pressing.until_armed(0).await;
        conn.item(sub, &item(13, shell_completed(0))).await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 0, "typeahead stays for the user's shell");
    let last_frame = out.rsplit("\x1b[?2026h").next().unwrap_or_default();
    assert!(!last_frame.contains("Ctrl+"), "the hint is gone: {last_frame:?}");
}

#[tokio::test]
async fn without_keys_a_silent_call_offers_nothing() {
    let env = TestEnv::new();
    let quit = Arc::new(TestQuit::default());
    let clock = Arc::new(GateClock::default());
    let ctx = Context { quit: quit.clone(), clock: clock.clone(), ..env.context() };
    let (result, _, err) = run_view(&env, &ctx, raw_view(), |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("./deploy"))).await;
        conn.item(sub, &item(12, shell_completed(0))).await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!err.contains("Ctrl+"), "{err}");
    assert_eq!(quit.armed(), 0);
}

/// A reader whose queue holds `queued`, as if those keys were typed and not read yet.
fn reader_with(queued: &[u8]) -> (KeyReader, tokio::sync::mpsc::Sender<u8>) {
    let (sender, keys) = tokio::sync::mpsc::channel(16);
    for key in queued {
        sender.try_send(*key).unwrap();
    }
    (KeyReader::from_channel(keys), sender)
}

fn pending(text: &str) -> Asking {
    let mut line = AnswerLine::new();
    for key in text.bytes() {
        super::pend(&mut line, key);
    }
    Asking::Pending { call_id: call(), line }
}

fn line_of(asking: &Asking) -> &str {
    match asking {
        Asking::Input { line, .. } | Asking::Pending { line, .. } => line.text(),
        other => panic!("no line: {other:?}"),
    }
}

#[test]
fn a_visible_wait_takes_the_pending_line_with_the_queued_keys_but_no_enter() {
    let (reader, _sender) = reader_with(b"es\r");
    let ask = Ask::Input { call_id: call(), kind: AnswerKind::Visible };
    let (mut reader, asking, seeded) = take_over(reader, Some(pending("Y\r")), ask);
    assert_eq!(line_of(&asking), "Yes");
    assert_eq!(seeded, Some(Seed::Shown("Yes".to_owned())), "a visible answer shows it");
    assert_eq!(reader.queued(), None, "the queue went into the line");
}

#[test]
fn a_secret_looking_wait_takes_the_pending_line_without_showing_it() {
    let (reader, _sender) = reader_with(b"2");
    let ask = Ask::Input { call_id: call(), kind: AnswerKind::Masked };
    let (_, asking, seeded) = take_over(reader, Some(pending("hunter")), ask);
    assert_eq!(line_of(&asking), "hunter2");
    assert_eq!(seeded, Some(Seed::Unshown(7)), "only how many characters it holds");
}

#[test]
fn a_hidden_wait_a_manual_line_or_another_call_drops_the_pending_line_and_the_queue() {
    let other: CallId = "0192f0c1-7a00-7000-8000-0000000000fd".parse().unwrap();
    for ask in [
        Ask::Input { call_id: call(), kind: AnswerKind::Hidden },
        Ask::Input { call_id: call(), kind: AnswerKind::Manual },
        Ask::Input { call_id: other, kind: AnswerKind::Visible },
        Ask::Discard(call()),
        Ask::Approval(other),
    ] {
        let (reader, _sender) = reader_with(b"more");
        let (mut reader, asking, seeded) = take_over(reader, Some(pending("ahead")), ask);
        assert_eq!(seeded, None, "{ask:?}");
        if let Asking::Input { line, .. } = &asking {
            assert_eq!(line.text(), "", "{ask:?}");
        }
        assert_eq!(reader.queued(), None, "{ask:?}");
    }
}

#[test]
fn keeping_the_keys_again_keeps_the_queue() {
    let (reader, _sender) = reader_with(b"y");
    let before =
        Asking::Input { call_id: call(), kind: AnswerKind::Visible, line: AnswerLine::new() };
    let (mut reader, asking, _) = take_over(reader, Some(before), Ask::Retain(call()));
    assert_eq!(line_of(&asking), "");
    assert_eq!(reader.queued(), Some(b'y'), "typed for the call, so it stays");
}

// --- frames, ticks, resizes and the ways out ----------------------------------------

#[test]
fn the_time_since_a_moment_is_never_negative() {
    let later = now() + jiff::SignedDuration::from_millis(250);
    assert_eq!(since_then(now(), later), std::time::Duration::from_millis(250));
    assert_eq!(since_then(later, now()), std::time::Duration::ZERO);
}

/// A terminal view with every switch on, whose status row runs, as `efr send` makes it.
fn started_view() -> TurnView {
    let look = Look { motion: true, summary: true, progress: true };
    let mut view = TurnView::new(turn(), RenderOptions::new(80)).with_look(look);
    view.start();
    view
}

/// How many frames `text` holds.
fn frames(text: &str) -> usize {
    text.matches("\x1b[?2026h").count()
}

fn updated(text: &str) -> Event {
    Event::AssistantMessageUpdated { turn_id: turn(), index: 0, offset: 0, delta: text.to_owned() }
}

/// A clock that moves only when the test moves it, shared with `ctx`.
fn test_clock(env: &TestEnv) -> (TestClock, Context) {
    let clock = TestClock::starting_at(now());
    let ctx = Context { clock: clock.shared(), ..env.context() };
    (clock, ctx)
}

#[tokio::test]
async fn a_burst_of_events_inside_one_frame_time_gives_one_frame() {
    let env = TestEnv::new();
    let (clock, ctx) = test_clock(&env);
    let timing = clock.clone();
    let (result, out, _) = run_view(&env, &ctx, started_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        // The first frame shows the status row; the tick waits.
        timing.wait_for_sleeps(1).await;
        let before = frames(&seen.stdout());
        let events = (0..50).map(|n| envelope(11 + n, updated(&"x".repeat(n as usize + 1))));
        let snapshot = ConversationSnapshot {
            conversation: ConversationSummary {
                id: conversation(),
                title: None,
                status: ConversationStatus::Running,
                created_at: now(),
                updated_at: now(),
                last_seq: Seq::new(60),
                cwd: None,
                scope: None,
                tty: None,
            },
            events: events.collect(),
            history_cursor: None,
            hwm: Seq::new(60),
        };
        conn.item(sub, &ConversationSubscribeItem::Snapshot(snapshot)).await;
        // The burst waits for the frame time.
        timing.wait_for_sleeps(2).await;
        assert_eq!(frames(&seen.stdout()), before, "nothing before the frame time is up");
        timing.advance(super::FRAME);
        shows_on(&seen, Stream::Stdout, |text| frames(text) == before + 1).await;
        assert!(seen.stdout().contains(&"x".repeat(50)));
        conn.item(sub, &item(61, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(
        out.matches(&"x".repeat(50)).count(),
        2,
        "the live text, then the committed text: {}",
        readable(&out)
    );
}

#[tokio::test]
async fn a_question_and_the_end_of_the_turn_never_wait_for_the_frame_time() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let (_clock, ctx) = test_clock(&env);
    let ctx = Context { keys: keys.clone(), ..ctx };
    let presser = Arc::clone(&keys);
    // The clock never moves: only a frame that goes out at once can show anything.
    let (result, out, _) = run_view(&env, &ctx, started_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, updated("Writing to ~/.zshrc."))).await;
        let request = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "write ~/.zshrc".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
        };
        conn.item(sub, &item(12, request)).await;
        shows(&seen, "\x1b[1my\x1b[0m\x1b[2m allow").await;
        assert!(seen.stdout().contains("Writing to"), "the question brings what came before it");
        presser.press(b'y').await;
        let (id, _) = conn.request().await;
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(13) }).await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(out.ends_with("\x1b[?25h\x1b]9;4;0\x1b\\"), "{}", readable(&out));
}

#[tokio::test]
async fn a_tick_moves_the_spinner_and_writes_only_the_status_row() {
    let env = TestEnv::new();
    let (clock, ctx) = test_clock(&env);
    let timing = clock.clone();
    let (result, _, _) = run_view(&env, &ctx, started_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        timing.wait_for_sleeps(1).await;
        let before = seen.stdout().len();
        timing.advance(super::TICK);
        shows_on(&seen, Stream::Stdout, |text| text.len() > before).await;
        let tick = seen.stdout()[before..].to_owned();
        assert!(
            tick.starts_with("\x1b[?2026h\r\x1b[1A\x1b[2K\x1b[33m\u{2819}"),
            "{}",
            readable(&tick)
        );
        assert!(tick.ends_with(progress::RUNNING), "the bar goes again: {}", readable(&tick));
        conn.item(sub, &item(11, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
}

#[tokio::test]
async fn a_resize_draws_the_live_zone_again_at_the_new_width() {
    let env = TestEnv::new();
    let resize = Arc::new(TestResize::default());
    let screen = Arc::new(ResizableScreen(std::sync::Mutex::new(Size { cols: 80, rows: 20 })));
    let ctx = Context { resize: resize.clone(), screen: screen.clone(), ..env.context() };
    let (result, _, _) = run_view(&env, &ctx, started_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, updated(&"word ".repeat(12)))).await;
        shows(&seen, "word word").await;
        let before = seen.stdout().len();
        // 59 columns of text took one row at 80 columns and take two at 40, above the
        // status row.
        screen.set(Size { cols: 40, rows: 20 });
        resize.trigger();
        shows_on(&seen, Stream::Stdout, |text| text.len() > before).await;
        let redraw = seen.stdout()[before..].to_owned();
        assert!(redraw.starts_with("\x1b[?2026h\r\x1b[3A\x1b[J"), "{}", readable(&redraw));
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
}

#[tokio::test]
async fn after_a_stop_the_live_zone_starts_again_below_the_shells_lines() {
    let env = TestEnv::new();
    let resume = Arc::new(TestResume::default());
    let ctx = Context { resume: resume.clone(), ..env.context() };
    let (result, _, _) = run_view(&env, &ctx, started_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, updated("Partial answer"))).await;
        shows(&seen, "Partial answer").await;
        let before = seen.stdout().len();
        // Ctrl+Z, then fg: the shell wrote its lines and showed the cursor.
        resume.trigger();
        shows_on(&seen, Stream::Stdout, |text| text.len() > before).await;
        let again = seen.stdout()[before..].to_owned();
        assert!(again.starts_with("\x1b[?25l\x1b[?2026hPartial answer"), "{}", readable(&again));
        assert!(!again.contains("A\x1b["), "nothing above the cursor moves: {}", readable(&again));
        assert!(again.ends_with(progress::RUNNING), "{}", readable(&again));
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
}

#[tokio::test]
async fn every_way_out_shows_the_cursor_and_clears_the_progress_bar() {
    #[derive(Debug, Clone, Copy)]
    enum Out {
        Completed,
        Failed,
        CtrlC,
        Ended,
        Sigterm,
    }
    for way in [Out::Completed, Out::Failed, Out::CtrlC, Out::Ended, Out::Sigterm] {
        let env = TestEnv::new();
        let interrupt = Arc::new(TestInterrupt::default());
        let terminate = Arc::new(TestTerminate::default());
        let ctx =
            Context { interrupt: interrupt.clone(), terminate: terminate.clone(), ..env.context() };
        let (result, out, _) = run_view(&env, &ctx, started_view(), |mut conn, seen| async move {
            let sub = subscribed(&mut conn, 10).await;
            conn.item(sub, &item(11, updated("Partial answer"))).await;
            shows(&seen, "Partial answer").await;
            match way {
                Out::Completed => conn.item(sub, &item(12, turn_completed())).await,
                Out::Failed => {
                    let error = ErrorBody::new(ErrorCode::Internal, "the provider is down");
                    conn.item(sub, &item(12, Event::TurnFailed { turn_id: turn(), error })).await;
                }
                Out::CtrlC => {
                    interrupt.trigger();
                    let (id, _) = request_after_cancels(&mut conn).await;
                    conn.reply(id, &TurnInterruptResult { turn_id: turn(), seq: Seq::new(12) })
                        .await;
                }
                Out::Ended => conn.end(sub).await,
                Out::Sigterm => terminate.trigger(),
            }
            conn.until_closed().await;
        })
        .await;
        assert_eq!(result.is_ok(), matches!(way, Out::Completed), "{way:?}: {result:?}");
        if matches!(way, Out::Sigterm) {
            assert!(matches!(result, Err(CliError::Ended { signal: 15 })), "{result:?}");
        }
        assert!(out.starts_with("\x1b[?25l"), "{way:?}: {}", readable(&out));
        let bar = if matches!(way, Out::Failed) { progress::FAILED } else { progress::CLEAR };
        let shown = out.rfind("\x1b[?25h").unwrap_or_else(|| panic!("{way:?}: {}", readable(&out)));
        assert!(out[shown..].contains(bar), "{way:?}: {}", readable(&out));
        assert!(!out[shown..].contains("\x1b[?25l"), "{way:?}: {}", readable(&out));
        assert_eq!(out.matches("\x1b[?25l").count(), 1, "{way:?}: {}", readable(&out));
    }
}

#[tokio::test]
async fn drafts_reach_the_screen_before_the_persisted_text_and_show_once() {
    let env = TestEnv::new();
    let (result, out, _) =
        run_view(&env, &env.context(), started_view(), |mut conn, seen| async move {
            let sub = subscribed(&mut conn, 10).await;
            let text = |offset: u64, delta: &str| Draft {
                turn_id: turn(),
                after_seq: Seq::new(10),
                draft: DraftPart::Text { index: 0, offset, delta: delta.to_owned() },
            };
            conn.item(sub, &ConversationSubscribeItem::Draft(text(0, "The disk "))).await;
            conn.item(sub, &ConversationSubscribeItem::Draft(text(9, "is full."))).await;
            shows(&seen, "The disk is full.").await;
            conn.item(sub, &item(11, updated("The disk is"))).await;
            let done = Event::AssistantMessageCompleted {
                turn_id: turn(),
                index: 0,
                text: "The disk is full.".to_owned(),
            };
            conn.item(sub, &item(12, done)).await;
            conn.item(sub, &item(13, turn_completed())).await;
            conn.until_closed().await;
        })
        .await;
    result.unwrap();
    assert_eq!(
        out.matches("The disk is full.").count(),
        2,
        "live, then committed once: {}",
        readable(&out)
    );
    assert!(!out.contains("The disk isThe"), "{}", readable(&out));
}
