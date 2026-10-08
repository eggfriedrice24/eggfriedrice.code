//! The input row against a fake daemon: what each key sends, what the view shows, the
//! prompts that the view follows after the turn, and the text that goes back to the
//! shell when `efr` ends.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, ApprovalRespondResult, ClientFrame, ConversationHistoryResult, ErrorBody,
    ErrorCode, Event, InputRespondResult, InputWait, LateSteer, Method, Mode, Origin, PromptSend,
    PromptSendResult, PromptWithdraw, PromptWithdrawResult, RequestId, ResentSteers, Scope, Seq,
    ShellContext, TurnId, TurnInterrupt, TurnInterruptResult, TurnSettings, TurnSteer,
    TurnSteerResult, WithdrawTarget, WithdrawnPrompt, WithdrawnSteer,
};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use super::super::{Compose, Row, TurnView, follow};
use super::{
    Stream, completed, input_changed, input_respond, readable, request_after_cancels,
    shell_completed, shell_output, shell_started, shows_on, started_view, subscribed, target,
    turn_completed,
};
use crate::cli::LastCommand;
use crate::context::Context;
use crate::error::CliError;
use crate::keys::Keys as _;
use crate::testing::{
    Captured, Conn, FakeDaemon, ScriptedKeys, TestEnv, TestInterrupt, bare, call, capture,
    conversation, item, turn,
};

/// The note when Esc sent the unread steers again as a new prompt.
const RESENT: &str = "interrupted to send your message";

/// Waits until stdout, without its escape sequences, holds `text`.
async fn shows(seen: &Captured, text: &str) {
    shows_where(seen, |shown| shown.contains(text)).await;
}

/// Waits until stdout, without its escape sequences, passes `test`.
async fn shows_where(seen: &Captured, test: impl Fn(&str) -> bool) {
    shows_on(seen, Stream::Stdout, |out| test(&bare(out))).await;
}

/// The second and the third turn of the conversation: prompts queued from the row.
fn turn_2() -> TurnId {
    "019a9b1c-3d00-7a10-8b20-000000000012".parse().unwrap()
}

fn turn_3() -> TurnId {
    "019a9b1c-3d00-7a10-8b20-000000000013".parse().unwrap()
}

/// What the plugin handed to the command: the row sends with it.
fn compose() -> Compose {
    let mut context = ShellContext::new("/home/user/project");
    context.tty = Some("/dev/pts/7".to_owned());
    Compose {
        context,
        last_command: Some(LastCommand::from("cargo test".to_owned())),
        settings: TurnSettings { mode: Some(Mode::Auto), model: None, effort: None },
    }
}

/// A test's keys, a context that reads them and a file for the text handed back.
struct Setup {
    env: TestEnv,
    keys: Arc<ScriptedKeys>,
    interrupt: Arc<TestInterrupt>,
    dir: tempfile::TempDir,
}

impl Setup {
    fn new() -> Setup {
        Setup {
            env: TestEnv::new(),
            keys: Arc::new(ScriptedKeys::default()),
            interrupt: Arc::new(TestInterrupt::default()),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn draft_file(&self) -> PathBuf {
        self.dir.path().join("drafts").join("4242")
    }

    /// The context with the plugin's file for the text, or without it.
    fn context(&self, plugin: bool) -> Context {
        let env = if plugin {
            Env::fixed([(Var::DraftFile, self.draft_file().into_os_string())])
        } else {
            Env::fixed(Vec::<(Var, String)>::new())
        };
        Context {
            keys: self.keys.clone(),
            interrupt: self.interrupt.clone(),
            env,
            ..self.env.context()
        }
    }

    /// What the command handed back to the plugin, if anything.
    fn handed_back(&self) -> Option<String> {
        std::fs::read_to_string(self.draft_file()).ok()
    }
}

/// Runs `follow` with the input row, against the fake daemon, which runs `script`.
async fn run_row<F>(
    setup: &Setup,
    ctx: &Context,
    script: impl FnOnce(Conn, Captured) -> F,
) -> (Result<(), CliError>, String, String)
where
    F: Future<Output = ()>,
{
    run_row_view(setup, ctx, started_view().with_input(), script).await
}

/// Runs `follow` of `view` with the input row, against the fake daemon, which runs
/// `script`.
async fn run_row_view<F>(
    setup: &Setup,
    ctx: &Context,
    mut view: TurnView,
    script: impl FnOnce(Conn, Captured) -> F,
) -> (Result<(), CliError>, String, String)
where
    F: Future<Output = ()>,
{
    let daemon = setup.env.listen();
    let (mut out, captured) = capture();
    let seen = captured.clone();
    let reader = setup.keys.keep().unwrap();
    let client = async {
        let client = ctx.connect(Origin::Shell, None).await.unwrap();
        let row = Row { compose: compose(), reader, origin: Origin::Shell };
        follow(ctx, &client, &mut out, &mut view, target(), Some(row)).await
    };
    let daemon = async { script(daemon.accept().await, seen).await };
    let (result, ()) = tokio::join!(client, daemon);
    (result, bare(&captured.stdout()), captured.stdout())
}

/// The next request, which must be `turn.steer`.
async fn steer_request(conn: &mut Conn) -> (RequestId, TurnSteer) {
    let (id, method) = conn.request().await;
    let Method::TurnSteer(params) = method else {
        panic!("expected turn.steer, got {}", method.name());
    };
    (id, params)
}

/// The next request, which must be `prompt.send`.
async fn send_request(conn: &mut Conn) -> (RequestId, PromptSend) {
    let (id, method) = conn.request().await;
    let Method::PromptSend(params) = method else {
        panic!("expected prompt.send, got {}", method.name());
    };
    (id, params)
}

/// The next request, which must be `turn.interrupt`, after the cancel of the
/// subscription that Ctrl+C dropped.
async fn interrupt_request(conn: &mut Conn) -> (RequestId, TurnInterrupt) {
    let (id, method) = request_after_cancels(conn).await;
    let Method::TurnInterrupt(params) = method else {
        panic!("expected turn.interrupt, got {}", method.name());
    };
    (id, params)
}

/// The next request, which must be `prompt.withdraw`.
async fn withdraw_request(conn: &mut Conn) -> (RequestId, PromptWithdraw) {
    let (id, method) = conn.request().await;
    let Method::PromptWithdraw(params) = method else {
        panic!("expected prompt.withdraw, got {}", method.name());
    };
    (id, params)
}

/// Queues `text` with Tab, and the daemon queues it as `turn`.
async fn tab(conn: &mut Conn, keys: &ScriptedKeys, text: &str, turn: TurnId, seq: u64) {
    keys.type_bytes(format!("{text}\t").as_bytes()).await;
    let (id, params) = send_request(conn).await;
    assert_eq!(params.text, text);
    let result = PromptSendResult {
        conversation_id: conversation(),
        turn_id: turn,
        seq: Seq::new(seq),
        queued: true,
        settings: None,
    };
    conn.reply(id, &result).await;
}

/// Steers with `text` and Enter, and the daemon records it as `seq`.
async fn enter(conn: &mut Conn, keys: &ScriptedKeys, text: &str, seq: u64) {
    keys.type_bytes(format!("{text}\r").as_bytes()).await;
    let (id, params) = steer_request(conn).await;
    assert_eq!(params.text, text);
    conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(seq), queued: false }).await;
}

fn started(turn_id: TurnId) -> Event {
    Event::TurnStarted {
        turn_id,
        cwd: PathBuf::from("/home/user/project"),
        scope: Scope::Machine,
        settings: None,
    }
}

fn answer(turn_id: TurnId, text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id, index: 0, text: text.to_owned() }
}

fn ended(turn_id: TurnId) -> Event {
    Event::TurnCompleted { turn_id, usage: None, changes: None }
}

/// The part of `text` after the last time it showed `marker`.
fn after<'a>(text: &'a str, marker: &str) -> &'a str {
    text.rfind(marker).map_or("", |at| &text[at..])
}

#[tokio::test]
async fn enter_steers_and_the_steer_waits_until_a_model_call_reads_it() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        // The row shows from the first frame, with its hint.
        shows(&seen, "enter steer").await;
        // Enter on an empty row sends nothing.
        keys.press(b'\r').await;
        keys.type_bytes(b"use the release build\r").await;
        let (id, params) = steer_request(&mut conn).await;
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.turn_id, Some(turn()));
        assert_eq!(params.text, "use the release build");
        let Some(LateSteer::Queue { context, last_command, settings }) = params.if_late else {
            panic!("a late steer is queued: {:?}", params.if_late);
        };
        assert_eq!(context.and_then(|context| context.tty).as_deref(), Some("/dev/pts/7"));
        assert_eq!(last_command.as_deref(), Some("cargo test"));
        assert_eq!(settings.mode, Some(Mode::Auto));
        conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(11), queued: false })
            .await;
        shows(&seen, "\u{21b3} steer: use the release build").await;
        let steered =
            Event::TurnSteered { turn_id: turn(), text: "use the release build".to_owned() };
        conn.item(sub, &item(11, steered)).await;
        let delivered = Event::SteeringDelivered { turn_id: turn(), steers: vec![Seq::new(11)] };
        conn.item(sub, &item(12, delivered)).await;
        shows(&seen, "> use the release build").await;
        conn.item(sub, &item(13, completed("Built in release."))).await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!out.contains("steered:"), "a steer of this view is no note: {}", readable(&out));
    let tail = after(&out, "> use the release build");
    assert!(!tail.contains("\u{21b3} steer"), "a steer that was read leaves the list");
    assert_eq!(setup.handed_back(), None, "nothing was left in the row");
}

#[tokio::test]
async fn tab_queues_a_prompt_and_the_view_follows_it_after_the_turn() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        keys.type_bytes(b"then update the docs\t").await;
        let (id, params) = send_request(&mut conn).await;
        assert_eq!(params.conversation_id, Some(conversation()));
        assert!(!params.new_conversation);
        assert_eq!(params.text, "then update the docs");
        assert_eq!(params.context.and_then(|context| context.tty).as_deref(), Some("/dev/pts/7"));
        assert_eq!(params.last_command.as_deref(), Some("cargo test"));
        assert_eq!(params.settings.mode, Some(Mode::Auto));
        let queued = PromptSendResult {
            conversation_id: conversation(),
            turn_id: turn_2(),
            seq: Seq::new(11),
            queued: true,
            settings: None,
        };
        conn.reply(id, &queued).await;
        shows(&seen, "\u{21b3} queued: then update the docs").await;
        conn.item(sub, &item(12, completed("First answer."))).await;
        conn.item(sub, &item(13, turn_completed())).await;
        // The command goes on: the queued prompt runs next.
        shows(&seen, "waiting for the running turn").await;
        conn.item(sub, &item(14, started(turn_2()))).await;
        shows(&seen, "> then update the docs").await;
        conn.item(sub, &item(15, answer(turn_2(), "Docs updated."))).await;
        conn.item(sub, &item(16, ended(turn_2()))).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    let second = after(&out, "> then update the docs");
    assert!(second.contains("Docs updated."), "{}", readable(&out));
}

#[tokio::test]
async fn a_steer_that_comes_too_late_waits_in_the_queue_with_a_note() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        keys.type_bytes(b"and run it again\r").await;
        let (id, _) = steer_request(&mut conn).await;
        conn.reply(id, &TurnSteerResult { turn_id: turn_2(), seq: Seq::new(12), queued: true })
            .await;
        shows(&seen, "\u{21b3} queued: and run it again (too late to steer").await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.item(sub, &item(14, started(turn_2()))).await;
        conn.item(sub, &item(15, ended(turn_2()))).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
}

#[tokio::test]
async fn esc_interrupts_resends_unread_steers_and_pulls_back_queued_prompts() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sleep 60"))).await;
        shows(&seen, "$ sleep 60").await;
        enter(&mut conn, &keys, "look at the logs first", 12).await;
        tab(&mut conn, &keys, "then fix the bug", turn_2(), 13).await;
        keys.type_bytes(b"half typed").await;
        shows(&seen, "half typed").await;
        keys.press_esc().await;
        let (id, params) = interrupt_request(&mut conn).await;
        assert_eq!(params.turn_id, Some(turn()));
        assert_eq!(params.resend_steers, [Seq::new(12)]);
        let compose = compose();
        let resend_as = LateSteer::Queue {
            context: Some(compose.context),
            last_command: Some("cargo test".to_owned()),
            settings: compose.settings,
        };
        assert_eq!(params.resend_as, Some(Box::new(resend_as)), "this terminal's values");
        assert!(params.withdraw_steers.is_empty(), "Esc takes back no steer");
        assert_eq!(params.withdraw, [turn_2()]);
        let result = TurnInterruptResult {
            turn_id: turn(),
            seq: Seq::new(14),
            resent: Some(ResentSteers {
                turn_id: turn_3(),
                seq: Seq::new(16),
                steers: vec![Seq::new(12)],
            }),
            withdrawn: vec![WithdrawnPrompt {
                turn_id: turn_2(),
                seq: Seq::new(15),
                text: "then fix the bug".to_owned(),
            }],
            withdrawn_steers: Vec::new(),
        };
        conn.reply(id, &result).await;
        shows(&seen, "  then fix the bug").await;
        // The same frame shows the result: the note waits for the stopped call.
        assert!(!bare(&seen.stdout()).contains(RESENT), "{}", bare(&seen.stdout()));
        let requested = Event::TurnInterruptRequested { turn_id: turn(), origin: Origin::Shell };
        conn.item(sub, &item(14, requested)).await;
        let withdrawn = Event::PromptWithdrawn { turn_id: turn_2(), origin: Origin::Shell };
        conn.item(sub, &item(15, withdrawn)).await;
        conn.item(sub, &item(17, shell_completed(130))).await;
        conn.item(sub, &item(18, Event::TurnInterrupted { turn_id: turn() })).await;
        shows(&seen, RESENT).await;
        // The steer runs as its own prompt now, which the view follows.
        conn.item(sub, &item(19, started(turn_3()))).await;
        shows(&seen, "> look at the logs first").await;
        conn.item(sub, &item(20, ended(turn_3()))).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    // In the order that it happened: the stopped call, the end of its turn, the note.
    let call_at = out.rfind("$ sleep 60").unwrap();
    let end_at = out.find("interrupted").unwrap();
    let note_at = out.find(RESENT).unwrap();
    assert!(call_at < end_at && end_at < note_at, "{}", readable(&out));
    assert_eq!(out.matches(RESENT).count(), 1, "{}", readable(&out));
    assert!(!after(&out, "> look at the logs first").contains("\u{21b3}"), "{}", readable(&out));
    assert!(!out.contains("interrupt requested"), "its own request needs no note: {out}");
    assert_eq!(setup.handed_back().as_deref(), Some("half typed\nthen fix the bug"));
}

#[tokio::test]
async fn esc_with_nothing_unread_ends_as_ctrl_c_does_and_the_text_shows_without_the_plugin() {
    let setup = Setup::new();
    let ctx = setup.context(false);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        keys.type_bytes(b"not now").await;
        shows(&seen, "not now").await;
        keys.press_esc().await;
        let (id, params) = interrupt_request(&mut conn).await;
        assert_eq!((params.resend_steers.len(), params.withdraw.len()), (0, 0));
        let result = TurnInterruptResult {
            turn_id: turn(),
            seq: Seq::new(11),
            resent: None,
            withdrawn: vec![],
            withdrawn_steers: Vec::new(),
        };
        conn.reply(id, &result).await;
        conn.item(sub, &item(12, Event::TurnInterrupted { turn_id: turn() })).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Escaped)), "{result:?}");
    assert!(out.contains("not sent: not now"), "{}", readable(&out));
}

#[tokio::test]
async fn alt_up_takes_back_the_newest_queued_prompt() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        tab(&mut conn, &keys, "first", turn_2(), 11).await;
        tab(&mut conn, &keys, "second", turn_3(), 12).await;
        keys.type_bytes(b"\x1b[1;3A").await;
        let (id, params) = withdraw_request(&mut conn).await;
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.target, WithdrawTarget::Turn { turn_id: turn_3() });
        let withdrawn =
            WithdrawnPrompt { turn_id: turn_3(), seq: Seq::new(13), text: "second".to_owned() };
        conn.reply(id, &PromptWithdrawResult { withdrawn }).await;
        shows(&seen, "\u{203a} second").await;
        // The next one already started: the daemon refuses, and the view keeps it, so
        // it follows the prompt that now runs until it ends.
        keys.type_bytes(b"\x1b[1;3A").await;
        let (id, params) = withdraw_request(&mut conn).await;
        assert_eq!(params.target, WithdrawTarget::Turn { turn_id: turn_2() });
        conn.fail(id, ErrorBody::new(ErrorCode::Conflict, "the prompt started")).await;
        shows(&seen, "not taken back: the prompt already started").await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.item(sub, &item(15, started(turn_2()))).await;
        conn.item(sub, &item(16, answer(turn_2(), "First done."))).await;
        shows(&seen, "First done.").await;
        let done = Event::TurnCompleted { turn_id: turn_2(), usage: None, changes: None };
        conn.item(sub, &item(17, done)).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(setup.handed_back().as_deref(), Some("second"));
}

#[tokio::test]
async fn ctrl_c_clears_the_row_first_then_interrupts_and_takes_back_the_queue() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let interrupt = Arc::clone(&setup.interrupt);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let _sub = subscribed(&mut conn, 10).await;
        enter(&mut conn, &keys, "a steer nobody reads", 11).await;
        tab(&mut conn, &keys, "queued", turn_2(), 12).await;
        keys.type_bytes(b"throw this away").await;
        shows(&seen, "throw this away").await;
        interrupt.trigger();
        shows_where(&seen, |out| after(out, "throw this away").contains("enter steer")).await;
        interrupt.trigger();
        let (id, params) = interrupt_request(&mut conn).await;
        assert_eq!(params.withdraw, [turn_2()]);
        assert!(params.resend_steers.is_empty(), "Ctrl+C sends nothing again");
        assert_eq!(params.resend_as, None);
        assert_eq!(params.withdraw_steers, [Seq::new(11)], "the daemon takes the steer back");
        let withdrawn =
            WithdrawnPrompt { turn_id: turn_2(), seq: Seq::new(14), text: "queued".to_owned() };
        let steer = WithdrawnSteer { seq: Seq::new(11), text: "a steer nobody reads".to_owned() };
        let result = TurnInterruptResult {
            turn_id: turn(),
            seq: Seq::new(13),
            resent: None,
            withdrawn: vec![withdrawn],
            withdrawn_steers: vec![steer],
        };
        conn.reply(id, &result).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert_eq!(setup.handed_back().as_deref(), Some("a steer nobody reads\nqueued"));
}

/// The review finding: Ctrl+C handed back every steer that the view still listed as
/// unread, also one that a model call had read just before. Only the steers that the
/// daemon took back come back now.
#[tokio::test]
async fn ctrl_c_hands_back_only_the_steers_that_the_daemon_took_back() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let interrupt = Arc::clone(&setup.interrupt);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, _| async move {
        let _sub = subscribed(&mut conn, 10).await;
        enter(&mut conn, &keys, "use tabs", 11).await;
        enter(&mut conn, &keys, "keep it small", 12).await;
        interrupt.trigger();
        let (id, params) = interrupt_request(&mut conn).await;
        assert_eq!(params.withdraw_steers, [Seq::new(11), Seq::new(12)]);
        // A model call read the first one before the view saw `steering_delivered`.
        let steer = WithdrawnSteer { seq: Seq::new(12), text: "keep it small".to_owned() };
        let result = TurnInterruptResult {
            turn_id: turn(),
            seq: Seq::new(14),
            resent: None,
            withdrawn: Vec::new(),
            withdrawn_steers: vec![steer],
        };
        conn.reply(id, &result).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert_eq!(setup.handed_back().as_deref(), Some("keep it small"));
}

/// A view whose followed prompt waits behind another terminal's turn.
fn queued_view() -> TurnView {
    let mut view = started_view().with_input();
    view.queue();
    view
}

/// Answers the read of the approvals that a queued prompt waits behind: none.
async fn nothing_blocks(conn: &mut Conn) {
    let (id, method) = conn.request().await;
    let Method::ConversationHistory(_) = method else {
        panic!("expected conversation.history, got {}", method.name());
    };
    conn.reply(id, &ConversationHistoryResult { events: Vec::new(), next_cursor: None }).await;
}

/// The review finding: Esc on a followed prompt that waits sent one withdraw per prompt,
/// the followed one first, and a followed prompt that started in between ran on. Now
/// the newest goes first, so none starts in between, and a followed prompt that
/// started is interrupted.
#[tokio::test]
async fn esc_on_a_waiting_prompt_takes_back_the_newest_first_and_interrupts_one_that_started() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, _, _) = run_row_view(&setup, &ctx, queued_view(), |mut conn, _| async move {
        nothing_blocks(&mut conn).await;
        let sub = subscribed(&mut conn, 10).await;
        tab(&mut conn, &keys, "then the docs", turn_2(), 11).await;
        keys.press_esc().await;
        let (id, params) = withdraw_request(&mut conn).await;
        assert_eq!(params.target, WithdrawTarget::Turn { turn_id: turn_2() }, "newest first");
        let withdrawn =
            WithdrawnPrompt { turn_id: turn_2(), seq: Seq::new(13), text: "then the docs".into() };
        conn.reply(id, &PromptWithdrawResult { withdrawn }).await;
        let (id, params) = withdraw_request(&mut conn).await;
        assert_eq!(params.target, WithdrawTarget::Turn { turn_id: turn() });
        conn.fail(id, ErrorBody::new(ErrorCode::Conflict, "the prompt started")).await;
        let (id, params) = interrupt_request(&mut conn).await;
        assert_eq!(params.turn_id, Some(turn()));
        assert!(params.withdraw.is_empty(), "the other prompt is back already");
        let result = TurnInterruptResult {
            turn_id: turn(),
            seq: Seq::new(14),
            resent: None,
            withdrawn: Vec::new(),
            withdrawn_steers: Vec::new(),
        };
        conn.reply(id, &result).await;
        conn.item(sub, &item(12, started(turn()))).await;
        conn.item(sub, &item(15, Event::TurnInterrupted { turn_id: turn() })).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Escaped)), "{result:?}");
    assert_eq!(setup.handed_back().as_deref(), Some("then the docs"));
}

/// The review finding: Ctrl+C on a followed prompt that waits sent `turn.interrupt`
/// for a turn that did not run, took back nothing and left the prompts of this view to
/// run with nobody to follow them. Now it takes them back, as Esc does.
#[tokio::test]
async fn ctrl_c_on_a_waiting_prompt_takes_it_back_with_the_prompts_behind_it() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let interrupt = Arc::clone(&setup.interrupt);
    let (result, out, _) = run_row_view(&setup, &ctx, queued_view(), |mut conn, _| async move {
        nothing_blocks(&mut conn).await;
        let _sub = subscribed(&mut conn, 10).await;
        tab(&mut conn, &keys, "then the docs", turn_2(), 11).await;
        interrupt.trigger();
        let (id, method) = request_after_cancels(&mut conn).await;
        let Method::PromptWithdraw(params) = method else {
            panic!("expected prompt.withdraw, got {}", method.name());
        };
        assert_eq!(params.target, WithdrawTarget::Turn { turn_id: turn_2() });
        let withdrawn =
            WithdrawnPrompt { turn_id: turn_2(), seq: Seq::new(12), text: "then the docs".into() };
        conn.reply(id, &PromptWithdrawResult { withdrawn }).await;
        let (id, params) = withdraw_request(&mut conn).await;
        assert_eq!(params.target, WithdrawTarget::Turn { turn_id: turn() });
        let withdrawn =
            WithdrawnPrompt { turn_id: turn(), seq: Seq::new(13), text: "write the code".into() };
        conn.reply(id, &PromptWithdrawResult { withdrawn }).await;
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert!(out.contains("the prompt was taken back before it ran"), "{}", readable(&out));
    assert_eq!(setup.handed_back().as_deref(), Some("write the code\nthen the docs"));
}

/// The review finding: when the connection ended before the answer to Ctrl+C, the
/// texts that the daemon may have taken back were lost. Without an answer, what the
/// view sent comes back to the shell with a note.
#[tokio::test]
async fn ctrl_c_without_an_answer_gives_back_the_unread_steers() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let interrupt = Arc::clone(&setup.interrupt);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, _| async move {
        let _sub = subscribed(&mut conn, 10).await;
        enter(&mut conn, &keys, "keep it small", 11).await;
        interrupt.trigger();
        let (_, params) = interrupt_request(&mut conn).await;
        assert_eq!(params.withdraw_steers, [Seq::new(11)]);
        drop(conn);
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert!(out.contains("the interrupt failed"), "{}", readable(&out));
    assert!(out.contains("goes back to your shell"), "{}", readable(&out));
    assert_eq!(setup.handed_back().as_deref(), Some("keep it small"));
}

/// A clock whose interrupt wait ends at once, as when the daemon answers too late.
#[derive(Debug)]
struct LateClock;

impl efr_stdx::time::Clock for LateClock {
    fn now(&self) -> jiff::Timestamp {
        crate::testing::now()
    }

    fn sleep(&self, duration: std::time::Duration) -> efr_stdx::time::Sleep {
        if duration <= super::super::FRAME || duration == super::super::INTERRUPT_TIMEOUT {
            return Box::pin(std::future::ready(()));
        }
        Box::pin(std::future::pending())
    }
}

/// The review finding: a daemon that took the steers back after the wait for its
/// answer ended kept them from every model call, and the view did not give them back.
/// Without an answer in time, they come back to the shell.
#[tokio::test]
async fn ctrl_c_whose_answer_comes_too_late_gives_back_the_unread_steers() {
    let setup = Setup::new();
    let ctx = Context { clock: Arc::new(LateClock), ..setup.context(true) };
    let keys = Arc::clone(&setup.keys);
    let interrupt = Arc::clone(&setup.interrupt);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, _| async move {
        let _sub = subscribed(&mut conn, 10).await;
        enter(&mut conn, &keys, "keep it small", 11).await;
        interrupt.trigger();
        conn.until_closed().await;
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert!(out.contains("did not confirm the interrupt"), "{}", readable(&out));
    assert_eq!(setup.handed_back().as_deref(), Some("keep it small"));
}

/// The review finding: Ctrl+C on a waiting prompt lost the prompts that the daemon took
/// back when the connection ended before the last answer. They come back now.
#[tokio::test]
async fn ctrl_c_on_a_waiting_prompt_keeps_what_was_taken_back_when_the_connection_ends() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let interrupt = Arc::clone(&setup.interrupt);
    let (result, out, _) = run_row_view(&setup, &ctx, queued_view(), |mut conn, _| async move {
        nothing_blocks(&mut conn).await;
        let _sub = subscribed(&mut conn, 10).await;
        tab(&mut conn, &keys, "then the docs", turn_2(), 11).await;
        interrupt.trigger();
        let (id, method) = request_after_cancels(&mut conn).await;
        let Method::PromptWithdraw(_) = method else {
            panic!("expected prompt.withdraw, got {}", method.name());
        };
        let withdrawn =
            WithdrawnPrompt { turn_id: turn_2(), seq: Seq::new(12), text: "then the docs".into() };
        conn.reply(id, &PromptWithdrawResult { withdrawn }).await;
        let (_, params) = withdraw_request(&mut conn).await;
        assert_eq!(params.target, WithdrawTarget::Turn { turn_id: turn() });
        drop(conn);
    })
    .await;
    assert!(matches!(result, Err(CliError::Interrupted)), "{result:?}");
    assert!(out.contains("goes back to your shell"), "{}", readable(&out));
    assert_eq!(setup.handed_back().as_deref(), Some("then the docs"));
}

#[tokio::test]
async fn a_question_takes_the_keys_and_the_row_keeps_its_text() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        keys.type_bytes(b"draft").await;
        shows(&seen, "draft").await;
        let request = Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "run rm -rf build".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: None,
        };
        conn.item(sub, &item(11, request)).await;
        shows(&seen, "y allow").await;
        keys.press(b'y').await;
        let (id, method) = conn.request().await;
        let Method::ApprovalRespond(params) = method else {
            panic!("expected approval.respond, got {}", method.name());
        };
        assert_eq!(params.decision, ApprovalDecision::Allow);
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(12) }).await;
        keys.type_bytes(b"!").await;
        shows(&seen, "\u{203a} draft!").await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    // While the question waited, the row was not shown.
    let asked = after(&out, "y allow");
    assert!(!asked[..asked.find("allowed").unwrap_or(asked.len())].contains("\u{203a}"));
    assert_eq!(setup.handed_back().as_deref(), Some("draft!"));
}

/// An approval of the followed turn for `call()`.
fn approval() -> Event {
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "run rm -rf build".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
    }
}

/// Reads the answer to the approval and checks that it is `decision`.
async fn approval_answer(conn: &mut Conn, decision: ApprovalDecision) {
    let (id, method) = conn.request().await;
    let Method::ApprovalRespond(params) = method else {
        panic!("expected approval.respond, got {}", method.name());
    };
    assert_eq!(params.decision, decision);
    conn.reply(id, &ApprovalRespondResult { seq: Seq::new(12) }).await;
}

/// The review finding: the keys still on their way when a question appeared answered
/// it. Now the keys typed before it (until the mark) go to the row, and a key that
/// would send stays text there.
#[tokio::test]
async fn keys_typed_before_a_question_go_to_the_row_and_never_answer_it() {
    let mut setup = Setup::new();
    setup.keys = Arc::new(ScriptedKeys::threaded());
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        keys.type_bytes(b"draft").await;
        shows(&seen, "draft").await;
        conn.item(sub, &item(11, approval())).await;
        shows(&seen, "y allow").await;
        // Typed before the question appeared: the row's.
        keys.type_bytes(b" no\r").await;
        keys.marked().await;
        keys.press(b'y').await;
        approval_answer(&mut conn, ApprovalDecision::Allow).await;
        keys.type_bytes(b"!").await;
        shows(&seen, "\u{203a} draft no!").await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(setup.handed_back().as_deref(), Some("draft no!"));
}

/// The review finding: a question that came in the middle of a paste took the rest of
/// the paste, and the row stayed in the paste for good. Now the rest of the paste goes
/// to the row, and the question takes the keys after it.
#[tokio::test]
async fn a_paste_that_a_question_cuts_goes_on_into_the_row() {
    let mut setup = Setup::new();
    setup.keys = Arc::new(ScriptedKeys::threaded());
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        keys.type_bytes(b"\x1b[200~say y").await;
        conn.item(sub, &item(11, approval())).await;
        shows(&seen, "y allow").await;
        keys.marked().await;
        // The rest of the paste: its `n` and its newline deny and send nothing.
        keys.type_bytes(b"es\nor no\x1b[201~").await;
        keys.press(b'y').await;
        approval_answer(&mut conn, ApprovalDecision::Allow).await;
        // Enter steers again once the keys are back: the row left the paste.
        keys.press(b'\r').await;
        let (id, params) = steer_request(&mut conn).await;
        assert_eq!(params.text, "say yes\nor no");
        conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(13), queued: false })
            .await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
}

#[tokio::test]
async fn after_a_password_the_keys_go_back_to_the_row_without_what_followed_it() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo true"))).await;
        conn.item(sub, &item(12, shell_output("[sudo] password for egg: "))).await;
        conn.item(sub, &item(13, input_changed(InputWait::Hidden))).await;
        shows(&seen, "it is not shown").await;
        keys.type_bytes(b"hunter2\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "hunter2");
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(14, input_changed(InputWait::None))).await;
        // Typed again while sudo checks it: thrown away.
        keys.type_bytes(b"hunter2").await;
        conn.item(sub, &item(15, shell_completed(0))).await;
        // The row comes back once the call ended.
        shows_where(&seen, |out| after(out, "\u{2713}").contains("enter steer")).await;
        keys.type_bytes(b"ok").await;
        shows(&seen, "\u{203a} ok").await;
        conn.item(sub, &item(16, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!out.contains("hunter"), "{}", readable(&out));
    assert_eq!(setup.handed_back().as_deref(), Some("ok"));
}

/// The review finding: a password typed for a prompt that the call's output showed,
/// before the daemon reported the wait, landed in the row, and Enter sent it to the
/// model as a steer. Now the keys wait for that prompt while its line shows: never
/// shown, never sent, and thrown away when the prompt goes.
#[tokio::test]
async fn a_password_typed_before_the_wait_is_reported_never_reaches_the_row() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("git push"))).await;
        let prompt = shell_output("Password for 'https://u@github.com': ");
        conn.item(sub, &item(12, prompt)).await;
        shows(&seen, "Password for").await;
        keys.type_bytes(b"hunter2\r").await;
        // The prompt goes without a wait: the keys go back to the row, without the
        // password.
        conn.item(sub, &item(13, shell_output("fatal: Authentication failed"))).await;
        shows_where(&seen, |out| after(out, "Authentication failed").contains("enter steer")).await;
        keys.type_bytes(b"ok").await;
        shows(&seen, "\u{203a} ok").await;
        conn.item(sub, &item(14, shell_completed(128))).await;
        conn.item(sub, &item(15, turn_completed())).await;
        let rest = conn.until_closed().await;
        assert!(
            rest.iter().all(|frame| matches!(frame, ClientFrame::Cancel { .. })),
            "no steer went out: {rest:?}"
        );
    })
    .await;
    result.unwrap();
    assert!(!out.contains("hunter"), "{}", readable(&out));
    assert_eq!(setup.handed_back().as_deref(), Some("ok"));
}

/// The review finding: every line of output that named a password held the keys, so a
/// steer typed while `grep password` ran was thrown away without a note. A line that
/// ends with a line break is not a prompt: the keys stay in the row, and Enter steers.
#[tokio::test]
async fn a_finished_line_that_names_a_password_does_not_hold_the_keys() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("grep -rn password config/"))).await;
        let found = shell_output("config/db.yml:3:password: ${DB_PASS}\n");
        conn.item(sub, &item(12, found)).await;
        shows(&seen, "DB_PASS").await;
        enter(&mut conn, &keys, "do not print secrets", 13).await;
        conn.item(sub, &item(14, shell_completed(0))).await;
        conn.item(sub, &item(15, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
}

/// Keys typed while a password prompt shows start its answer when the daemon then
/// reports a visible wait that looks secret, unshown, as for a call allowed here.
#[tokio::test]
async fn keys_held_for_a_password_prompt_start_its_secret_looking_answer() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out, _) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, shell_started("sudo -k true"))).await;
        conn.item(sub, &item(12, shell_output("[sudo] password for u: "))).await;
        shows(&seen, "[sudo] password").await;
        keys.type_bytes(b"hunter2\r").await;
        let wait = Event::ToolCallInputChanged {
            turn_id: turn(),
            call_id: call(),
            input: InputWait::Visible,
            looks_secret: true,
        };
        conn.item(sub, &item(13, wait)).await;
        shows(&seen, "starts with 7 characters typed ahead").await;
        keys.press(b'\r').await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.text.expose_secret(), "hunter2");
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(14, shell_completed(0))).await;
        conn.item(sub, &item(15, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!out.contains("hunter"), "{}", readable(&out));
    assert_eq!(setup.handed_back(), None);
}

#[tokio::test]
async fn the_cursor_shows_in_the_row_and_paste_mode_is_off_on_the_way_out() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, _, out) = run_row(&setup, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        keys.type_bytes(b"x").await;
        shows(&seen, "\u{203a} x").await;
        conn.item(sub, &item(11, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(out.starts_with("\x1b[?2004h"), "{}", readable(&out));
    assert!(!out.contains("\x1b[?25l"), "the cursor never hides: {}", readable(&out));
    let off = out.rfind("\x1b[?2004l").unwrap_or_else(|| panic!("{}", readable(&out)));
    assert!(!out[off..].contains("\u{203a}"), "the row is gone after it");
    assert_eq!(setup.handed_back().as_deref(), Some("x"));
}

/// The directory of the plugin's file exists only once a text was handed back.
#[tokio::test]
async fn a_turn_that_ends_with_an_empty_row_writes_no_file() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let (result, _, _) = run_row(&setup, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!Path::new(&setup.draft_file()).parent().unwrap().exists());
}

/// Runs `follow` with the input row against a fake daemon that the script accepts on
/// itself, so it can drop a connection and accept the next one, as efrd does when it
/// restarts.
async fn run_row_restarting<F>(
    setup: &Setup,
    ctx: &Context,
    script: impl FnOnce(FakeDaemon, Captured) -> F,
) -> (Result<(), CliError>, String)
where
    F: Future<Output = ()>,
{
    let daemon = setup.env.listen();
    let (mut out, captured) = capture();
    let seen = captured.clone();
    let mut view = started_view().with_input();
    let reader = setup.keys.keep().unwrap();
    let client = async {
        let client = ctx.connect(Origin::Shell, None).await.unwrap();
        let row = Row { compose: compose(), reader, origin: Origin::Shell };
        follow(ctx, &client, &mut out, &mut view, target(), Some(row)).await
    };
    let daemon = async { script(daemon, seen).await };
    let (result, ()) = tokio::join!(client, daemon);
    (result, bare(&captured.stdout()))
}

/// The review finding: when efrd restarted, only the text of the row went back to the
/// shell. The prompts that the view queued and its unread steers were lost without a
/// word, because the view never saw the `turn_cancelled` of the new efrd. Now the view
/// connects again, and the cancelled turns put their texts back.
#[tokio::test]
async fn after_a_restart_of_efrd_the_cancelled_prompts_and_unread_steers_come_back() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out) = run_row_restarting(&setup, &ctx, |daemon, seen| async move {
        let mut conn = daemon.accept().await;
        let sub = subscribed(&mut conn, 10).await;
        tab(&mut conn, &keys, "then write the tests", turn_2(), 11).await;
        enter(&mut conn, &keys, "keep it small", 12).await;
        // efrd stops: it ends the stream and closes the connection.
        conn.end(sub).await;
        drop(conn);
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().tty.as_deref(), Some("/dev/pts/7"), "the same terminal");
        let sub = subscribed(&mut conn, 10).await;
        // The new efrd cancelled the running turn and the queued one.
        conn.item(sub, &item(13, Event::TurnCancelled { turn_id: turn() })).await;
        conn.item(sub, &item(14, Event::TurnCancelled { turn_id: turn_2() })).await;
        conn.until_closed().await;
        shows(&seen, "connecting again").await;
    })
    .await;
    assert!(matches!(result, Err(CliError::TurnCancelled)), "{result:?}");
    assert!(out.contains("the connection to efrd ended; connecting again"), "{out}");
    assert_eq!(setup.handed_back().as_deref(), Some("keep it small\nthen write the tests"));
}

/// The review finding: a steer whose answer was lost with the connection went back to
/// the shell although efrd had recorded it, so it could go twice. Now the view connects
/// again and sends the same request, and efrd answers from its receipt.
#[tokio::test]
async fn a_steer_whose_answer_was_lost_goes_again_with_the_same_command_id() {
    let setup = Setup::new();
    let ctx = setup.context(true);
    let keys = Arc::clone(&setup.keys);
    let (result, out) = run_row_restarting(&setup, &ctx, |daemon, _| async move {
        let mut conn = daemon.accept().await;
        subscribed(&mut conn, 10).await;
        keys.type_bytes(b"use tabs\r").await;
        let (_, first) = steer_request(&mut conn).await;
        // efrd recorded the steer, and the connection broke before the answer.
        drop(conn);
        let mut conn = daemon.accept().await;
        let (id, again) = steer_request(&mut conn).await;
        assert_eq!(again.command_id, first.command_id, "a retry, which the receipt answers");
        assert_eq!(again.text, "use tabs");
        conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(11), queued: false })
            .await;
        let sub = subscribed(&mut conn, 10).await;
        let read = Event::SteeringDelivered { turn_id: turn(), steers: vec![Seq::new(11)] };
        conn.item(sub, &item(12, read)).await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(out.contains("> use tabs"), "{out}");
    assert_eq!(setup.handed_back(), None, "nothing goes back: efrd took the steer");
}

/// A clock whose sleeps never finish, except a frame's and the pause between two tries
/// to connect again, so the tries run at once.
#[derive(Debug)]
struct RetryClock;

impl efr_stdx::time::Clock for RetryClock {
    fn now(&self) -> jiff::Timestamp {
        crate::testing::now()
    }

    fn sleep(&self, duration: std::time::Duration) -> efr_stdx::time::Sleep {
        if duration <= super::super::FRAME || duration == super::super::RECONNECT_PAUSE {
            return Box::pin(std::future::ready(()));
        }
        Box::pin(std::future::pending())
    }
}

/// When efrd does not come back, what the view queued and steered goes back to the
/// shell with a note, because nothing says whether it will run.
#[tokio::test]
async fn when_efrd_does_not_come_back_what_the_view_sent_goes_back_to_the_shell() {
    let setup = Setup::new();
    let ctx = Context { clock: Arc::new(RetryClock), ..setup.context(true) };
    let keys = Arc::clone(&setup.keys);
    let (result, out) = run_row_restarting(&setup, &ctx, |daemon, _| async move {
        let mut conn = daemon.accept().await;
        subscribed(&mut conn, 10).await;
        tab(&mut conn, &keys, "then write the tests", turn_2(), 11).await;
        enter(&mut conn, &keys, "keep it small", 12).await;
        drop(daemon);
        drop(conn);
    })
    .await;
    assert!(matches!(result, Err(CliError::Client(_))), "{result:?}");
    assert!(out.contains("efrd did not come back"), "{out}");
    assert_eq!(setup.handed_back().as_deref(), Some("keep it small\nthen write the tests"));
}
