//! A command that a shell call left running in the hidden shell, against a fake
//! daemon: the answer goes with its call id, Ctrl+C at its line stops the command and
//! not the turn, and `efr` stays open after the turn while it waits for an answer here.

use std::sync::Arc;

use efr_protocol::{
    ApprovedBy, ClientFrame, ErrorBody, ErrorCode, Event, InputRespondResult, InputWait, Method,
    RequestId, ShellCommandEnd, ShellInterrupt, ShellInterruptResult,
};
use pretty_assertions::assert_eq;

use super::{
    input_respond, run_view, shell_output, shell_started, shows, subscribed, terminal_view,
    turn_completed,
};
use crate::context::Context;
use crate::testing::{
    Conn, ScriptedKeys, TestEnv, TestInterrupt, bare, call, conversation, item, now, turn,
};

const RSYNC: &str = "rsync -a -e ssh u@h:src/ dst/";

/// The call answered the model while its command still runs.
fn left_completed() -> Event {
    Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: call(),
        output: String::new(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: true,
    }
}

fn detached(input: InputWait) -> Event {
    Event::ShellCommandDetached {
        call_id: call(),
        command: RSYNC.to_owned(),
        since: now(),
        contained: false,
        input,
        looks_secret: false,
        prompt: "u@h's password: ".to_owned(),
        approved_by: Some(ApprovedBy::You),
    }
}

pub(super) fn wait_changed(input: InputWait) -> Event {
    Event::ShellCommandInputChanged {
        call_id: call(),
        input,
        contained: false,
        looks_secret: false,
        prompt: String::new(),
    }
}

/// The call of the turn times out while its command waits for a password, from `seq`.
pub(super) async fn leave_waiting(conn: &mut Conn, sub: RequestId, seq: u64) {
    conn.item(sub, &item(seq, shell_started(RSYNC))).await;
    conn.item(sub, &item(seq + 1, shell_output("u@h's password: "))).await;
    conn.item(sub, &item(seq + 2, left_completed())).await;
    conn.item(sub, &item(seq + 3, detached(InputWait::Hidden))).await;
}

/// The next request, which must be `shell.interrupt`.
async fn interrupt_left(conn: &mut Conn) -> (RequestId, ShellInterrupt) {
    let (id, method) = conn.request().await;
    let Method::ShellInterrupt(params) = method else {
        panic!("expected shell.interrupt, got {}", method.name());
    };
    (id, params)
}

#[tokio::test]
async fn the_answer_to_a_left_command_goes_with_its_call_id_and_never_shows() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, err) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        leave_waiting(&mut conn, sub, 11).await;
        shows(&seen, "still runs in the hidden shell").await;
        shows(&seen, "the hidden shell: rsync").await;
        presser.type_bytes(b"hunter2\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.call_id, call());
        assert_eq!(params.text.expose_secret(), "hunter2");
        assert!(params.hidden);
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(15, wait_changed(InputWait::None))).await;
        let end = Event::ShellCommandEnded {
            call_id: call(),
            exit_code: Some(0),
            how: ShellCommandEnd::Ended,
        };
        conn.item(sub, &item(16, end)).await;
        shows(&seen, "ended in the hidden shell").await;
        conn.item(sub, &item(17, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    for written in [&out, &err] {
        assert!(!written.contains("hunter"), "{written}");
    }
}

#[tokio::test]
async fn ctrl_c_at_the_line_of_a_left_command_stops_it_and_not_the_turn() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context { keys: keys.clone(), interrupt: interrupt.clone(), ..env.context() };
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        leave_waiting(&mut conn, sub, 11).await;
        shows(&seen, "ctrl+c stops the command, not the turn").await;
        interrupt.trigger();
        let (id, params) = interrupt_left(&mut conn).await;
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.call_id, call());
        assert!(!params.close);
        // The shell itself holds the terminal: a note, and the line stays.
        conn.fail(id, ErrorBody::new(ErrorCode::Conflict, "the shell itself holds the terminal"))
            .await;
        shows(&seen, "the command was not stopped: the shell itself holds the terminal").await;
        interrupt.trigger();
        let (id, _) = interrupt_left(&mut conn).await;
        conn.reply(id, &ShellInterruptResult {}).await;
        let end = Event::ShellCommandEnded {
            call_id: call(),
            exit_code: None,
            how: ShellCommandEnd::Stopped,
        };
        conn.item(sub, &item(15, end)).await;
        shows(&seen, "stopped in the hidden shell").await;
        conn.item(sub, &item(16, turn_completed())).await;
        let rest = conn.until_closed().await;
        assert!(
            rest.iter().all(|frame| matches!(frame, ClientFrame::Cancel { .. })),
            "no turn.interrupt went out: {rest:?}"
        );
    })
    .await;
    result.unwrap();
    assert!(!bare(&out).contains("interrupted"), "{}", bare(&out));
}

#[tokio::test]
async fn efr_stays_open_after_the_turn_while_a_left_command_waits_until_esc() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, out, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        leave_waiting(&mut conn, sub, 11).await;
        conn.item(sub, &item(15, turn_completed())).await;
        shows(&seen, "esc leaves it").await;
        presser.type_bytes(b"half").await;
        presser.press_esc().await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    let shown = bare(&out);
    assert!(shown.contains("the hidden shell still runs: rsync"), "{shown}");
    assert!(
        shown.contains("efr leaves the command waiting; ,shell answers or stops it"),
        "{shown}"
    );
    assert!(keys.discarded(), "the keys of the line go with it");
}

#[tokio::test]
async fn after_the_turn_an_answer_and_the_end_of_the_wait_end_efr() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, _, _) = run_view(&env, &ctx, terminal_view(), |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        leave_waiting(&mut conn, sub, 11).await;
        conn.item(sub, &item(15, turn_completed())).await;
        shows(&seen, "esc leaves it").await;
        presser.type_bytes(b"pw\r").await;
        let (id, params) = input_respond(&mut conn).await;
        assert_eq!(params.call_id, call());
        conn.reply(id, &InputRespondResult {}).await;
        conn.item(sub, &item(16, wait_changed(InputWait::None))).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
}
