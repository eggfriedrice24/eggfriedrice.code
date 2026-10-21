use std::sync::Arc;

use efr_protocol::{
    ApprovedBy, BusyShell, ConversationSubscribe, ErrorBody, ErrorCode, Event, InputRespond,
    InputRespondResult, InputWait, Method, RequestId, ShellCommandEnd, ShellInterrupt,
    ShellInterruptResult, ShellStatusResult,
};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;

use crate::cli::ShellArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::testing::{
    Captured, Conn, ScriptedKeys, TestEnv, TestInterrupt, TestTerminate, call, capture,
    conversation, item, now,
};

const RSYNC: &str = "rsync -a -e ssh u@h:src/ dst/";

fn busy(input: InputWait, contained: bool) -> BusyShell {
    BusyShell {
        call_id: call(),
        command: RSYNC.to_owned(),
        since: now(),
        contained,
        input,
        looks_secret: false,
        prompt: if input == InputWait::None {
            String::new()
        } else {
            "u@h's password: ".to_owned()
        },
        approved_by: Some(ApprovedBy::You),
    }
}

/// A test's keys and signals.
struct Setup {
    env: TestEnv,
    keys: Arc<ScriptedKeys>,
    interrupt: Arc<TestInterrupt>,
    terminate: Arc<TestTerminate>,
}

impl Setup {
    fn new() -> Setup {
        Setup {
            env: TestEnv::new(),
            keys: Arc::new(ScriptedKeys::default()),
            interrupt: Arc::new(TestInterrupt::default()),
            terminate: Arc::new(TestTerminate::default()),
        }
    }

    /// The context, with the scripted keys when `keys`, else with no terminal.
    fn context(&self, keys: bool) -> Context {
        let base = self.env.context();
        Context {
            keys: if keys { self.keys.clone() } else { base.keys.clone() },
            interrupt: self.interrupt.clone(),
            terminate: self.terminate.clone(),
            ..base
        }
    }
}

/// Runs `efr shell <conversation>` against the fake daemon, which runs `script` after
/// it answered `shell.status` with `status`.
async fn run<F>(
    setup: &Setup,
    keys: bool,
    status: ShellStatusResult,
    script: impl FnOnce(Conn, Captured) -> F,
) -> (Result<(), CliError>, String, String)
where
    F: Future<Output = ()>,
{
    let ctx = setup.context(keys);
    let daemon = setup.env.listen();
    let (mut out, captured) = capture();
    let seen = captured.clone();
    let args = ShellArgs { conversation: Some(conversation().to_string()) };
    let client = super::run(&ctx, &mut out, &args);
    let daemon = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::ShellStatus(params) = method else {
            panic!("expected shell.status, got {}", method.name());
        };
        assert_eq!(params.conversation_id, conversation());
        conn.reply(id, &status).await;
        script(conn, seen).await;
    };
    let (result, ()) = tokio::join!(client, daemon);
    (result, captured.stdout(), captured.stderr())
}

/// Reads the subscription, which can answer the command's input.
async fn subscribed(conn: &mut Conn) -> RequestId {
    let (id, method) = conn.request().await;
    let Method::ConversationSubscribe(ConversationSubscribe {
        conversation_id, answers_input, ..
    }) = method
    else {
        panic!("expected conversation.subscribe, got {}", method.name());
    };
    assert_eq!(conversation_id, conversation());
    assert!(answers_input, "a person here can answer");
    id
}

async fn shows(seen: &Captured, text: &str) {
    Wait::new(text).until(|| seen.stdout().contains(text)).await.unwrap();
}

async fn respond(conn: &mut Conn) -> (RequestId, InputRespond) {
    let (id, method) = conn.request().await;
    let Method::InputRespond(params) = method else {
        panic!("expected input.respond, got {}", method.name());
    };
    (id, params)
}

async fn interrupt(conn: &mut Conn) -> (RequestId, ShellInterrupt) {
    let (id, method) = conn.request().await;
    let Method::ShellInterrupt(params) = method else {
        panic!("expected shell.interrupt, got {}", method.name());
    };
    (id, params)
}

fn ended(how: ShellCommandEnd) -> Event {
    Event::ShellCommandEnded { call_id: call(), exit_code: None, how }
}

#[tokio::test]
async fn a_free_shell_says_so_and_subscribes_to_nothing() {
    let setup = Setup::new();
    let (result, out, _) =
        run(&setup, true, ShellStatusResult::default(), |mut conn, _| async move {
            assert!(conn.until_closed().await.is_empty(), "no other request");
        })
        .await;
    result.unwrap();
    assert_eq!(out, "the hidden shell is free\n");
    assert_eq!(setup.keys.starts(), 0, "no key is read");
}

#[tokio::test]
async fn a_wait_shows_its_prompt_row_and_the_answer_goes_hidden_with_its_call() {
    let setup = Setup::new();
    let keys = Arc::clone(&setup.keys);
    let status = ShellStatusResult { busy: Some(busy(InputWait::Hidden, false)) };
    let (result, out, err) = run(&setup, true, status, |mut conn, seen| async move {
        let sub = subscribed(&mut conn).await;
        shows(&seen, "u@h's password:").await;
        keys.type_bytes(b"hunter2\r").await;
        let (id, params) = respond(&mut conn).await;
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.call_id, call());
        assert_eq!(params.text.expose_secret(), "hunter2");
        assert!(params.hidden && !params.manual);
        conn.reply(id, &InputRespondResult {}).await;
        let none = Event::ShellCommandInputChanged {
            call_id: call(),
            input: InputWait::None,
            contained: false,
            looks_secret: false,
            prompt: String::new(),
        };
        conn.item(sub, &item(20, none)).await;
        shows(&seen, "it waits for no input now").await;
        conn.item(sub, &item(21, ended(ShellCommandEnd::Ended))).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    insta::assert_snapshot!(out);
    for written in [&out, &err] {
        assert!(!written.contains("hunter"), "{written}");
    }
    assert!(setup.keys.discarded(), "what was still unread goes before echo comes back");
}

#[tokio::test]
async fn ctrl_c_stops_the_command_and_a_second_asks_to_close_the_shell() {
    let setup = Setup::new();
    let keys = Arc::clone(&setup.keys);
    let signal = Arc::clone(&setup.interrupt);
    let status = ShellStatusResult { busy: Some(busy(InputWait::None, false)) };
    let (result, out, _) = run(&setup, true, status, |mut conn, seen| async move {
        let sub = subscribed(&mut conn).await;
        shows(&seen, "it waits for no input now").await;
        signal.trigger();
        let (id, params) = interrupt(&mut conn).await;
        assert_eq!((params.call_id, params.close), (call(), false));
        conn.reply(id, &ShellInterruptResult {}).await;
        shows(&seen, "ctrl+c again closes the hidden shell").await;
        signal.trigger();
        shows(&seen, "close the hidden shell? [y/n]").await;
        keys.press(b'y').await;
        let (id, params) = interrupt(&mut conn).await;
        assert!(params.close);
        conn.reply(id, &ShellInterruptResult {}).await;
        conn.item(sub, &item(20, ended(ShellCommandEnd::Closed))).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    insta::assert_snapshot!(out);
}

#[tokio::test]
async fn a_refused_stop_is_a_note_and_the_command_stays() {
    let setup = Setup::new();
    let keys = Arc::clone(&setup.keys);
    let signal = Arc::clone(&setup.interrupt);
    let status = ShellStatusResult { busy: Some(busy(InputWait::None, false)) };
    let (result, out, _) = run(&setup, true, status, |mut conn, seen| async move {
        subscribed(&mut conn).await;
        shows(&seen, "esc leaves it").await;
        signal.trigger();
        let (id, _) = interrupt(&mut conn).await;
        conn.fail(id, ErrorBody::new(ErrorCode::Conflict, "the shell itself holds the terminal"))
            .await;
        shows(&seen, "the command was not stopped: the shell itself holds the terminal").await;
        keys.press_esc().await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(out.ends_with("efr shell leaves it as it is; ,shell answers or stops it\n"), "{out}");
}

#[tokio::test]
async fn esc_leaves_the_command_and_the_typed_text_goes() {
    let setup = Setup::new();
    let keys = Arc::clone(&setup.keys);
    let status = ShellStatusResult { busy: Some(busy(InputWait::Hidden, false)) };
    let (result, out, _) = run(&setup, true, status, |mut conn, seen| async move {
        subscribed(&mut conn).await;
        shows(&seen, "esc leaves it").await;
        keys.type_bytes(b"half").await;
        keys.press_esc().await;
        let rest = conn.until_closed().await;
        assert!(
            rest.iter().all(|frame| matches!(frame, efr_protocol::ClientFrame::Cancel { .. })),
            "nothing was sent: {rest:?}"
        );
    })
    .await;
    result.unwrap();
    assert!(!out.contains("half"), "{out}");
    assert!(setup.keys.discarded());
}

#[tokio::test]
async fn the_terminal_modes_come_back_after_an_error_and_after_sigterm() {
    let setup = Setup::new();
    let status = ShellStatusResult { busy: Some(busy(InputWait::Hidden, false)) };
    let (result, _, _) = run(&setup, true, status, |mut conn, seen| async move {
        let sub = subscribed(&mut conn).await;
        shows(&seen, "esc leaves it").await;
        conn.fail(sub, ErrorBody::new(ErrorCode::Internal, "the store failed")).await;
        conn.until_closed().await;
    })
    .await;
    assert!(result.is_err(), "{result:?}");
    assert!(setup.keys.discarded(), "the key reader stopped and gave the terminal back");

    let setup = Setup::new();
    let terminate = Arc::clone(&setup.terminate);
    let status = ShellStatusResult { busy: Some(busy(InputWait::Hidden, false)) };
    let (result, _, _) = run(&setup, true, status, |mut conn, seen| async move {
        subscribed(&mut conn).await;
        shows(&seen, "esc leaves it").await;
        terminate.trigger();
        conn.until_closed().await;
    })
    .await;
    assert!(
        matches!(result, Err(CliError::Ended { signal: TestTerminate::SIGTERM })),
        "{result:?}"
    );
    assert!(setup.keys.discarded());
}

#[tokio::test]
async fn without_a_terminal_a_password_is_refused_and_a_question_is_a_note() {
    let setup = Setup::new();
    let status = ShellStatusResult { busy: Some(busy(InputWait::Hidden, false)) };
    let (result, out, _) = run(&setup, false, status, |mut conn, _| async move {
        assert!(conn.until_closed().await.is_empty(), "no subscription, no answer");
    })
    .await;
    assert!(matches!(result, Err(CliError::ShellNeedsTerminal)), "{result:?}");
    assert!(out.contains("u@h's password:"), "{out}");

    let setup = Setup::new();
    let status = ShellStatusResult { busy: Some(busy(InputWait::Visible, false)) };
    let (result, _, err) = run(&setup, false, status, |mut conn, _| async move {
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(err.contains("efr cannot ask for its input here"), "{err}");
}

#[tokio::test]
async fn a_password_prompt_in_the_sandbox_takes_no_answer() {
    let setup = Setup::new();
    let keys = Arc::clone(&setup.keys);
    let status = ShellStatusResult { busy: Some(busy(InputWait::Hidden, true)) };
    let (result, out, _) = run(&setup, true, status, |mut conn, seen| async move {
        let sub = subscribed(&mut conn).await;
        shows(&seen, "efr does not type secrets into the sandbox").await;
        keys.type_bytes(b"hunter2\r").await;
        conn.item(sub, &item(20, ended(ShellCommandEnd::Limit))).await;
        let rest = conn.until_closed().await;
        assert!(
            rest.iter().all(|frame| matches!(frame, efr_protocol::ClientFrame::Cancel { .. })),
            "no answer went out: {rest:?}"
        );
    })
    .await;
    result.unwrap();
    assert!(out.contains("(for 0.0s; in the sandbox; you approved it)"), "{out}");
    // NOTE: efr stopped the wait at once; the word password shows only outside the
    // sandbox (DS50).
    assert!(out.ends_with("it asked for a secret in the sandbox, where efr types none\n"), "{out}");
}
