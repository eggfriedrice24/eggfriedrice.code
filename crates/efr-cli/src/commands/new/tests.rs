use efr_protocol::{Event, Method, Mode, Origin, PromptSendResult, Seq, TurnSettings};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command, conversation, item, turn};

fn sent() -> PromptSendResult {
    PromptSendResult {
        conversation_id: conversation(),
        turn_id: turn(),
        seq: Seq::new(5),
        queued: false,
        settings: None,
    }
}

#[tokio::test]
async fn new_with_a_prompt_starts_a_conversation_and_follows_the_reply() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&[
        "new",
        "--context-json",
        r#"{"pwd":"/srv"}"#,
        "--last-command",
        "ls",
        "--",
        "start",
        "fresh",
    ]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::PromptSend(params) = method else { panic!("expected prompt.send") };
        conn.reply(id, &sent()).await;
        let (sub, _) = conn.request().await;
        let done = Event::AssistantMessageCompleted {
            turn_id: turn(),
            index: 0,
            text: "Fresh.".to_owned(),
        };
        conn.item(sub, &item(6, done)).await;
        conn.item(sub, &item(7, Event::TurnCompleted { turn_id: turn(), usage: None })).await;
        conn.until_closed().await;
        params
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert!(params.new_conversation);
    assert_eq!(params.conversation_id, None);
    assert_eq!(params.text, "start fresh");
    assert_eq!(params.last_command.as_deref(), Some("ls"));
    assert_eq!(captured.stdout(), "Fresh.\n");
}

#[tokio::test]
async fn new_reads_what_the_plugin_hands_over_in_the_environment() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let vars = [
        (Var::Context, r#"{"pwd":"/srv","tty":"/dev/pts/2"}"#),
        (Var::LastCommand, "ls"),
        (Var::Prompt, "start fresh"),
    ];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Shell);
        let (id, method) = conn.request().await;
        let Method::PromptSend(params) = method else { panic!("expected prompt.send") };
        conn.reply(id, &sent()).await;
        let (sub, _) = conn.request().await;
        conn.item(sub, &item(6, Event::TurnCompleted { turn_id: turn(), usage: None })).await;
        conn.until_closed().await;
        params
    };
    let line = command(&["new"]);
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert!(params.new_conversation);
    assert_eq!(params.text, "start fresh");
    assert_eq!(params.last_command.as_deref(), Some("ls"));
    assert_eq!(params.context.unwrap().tty.as_deref(), Some("/dev/pts/2"));
}

#[tokio::test]
async fn new_without_a_prompt_is_a_usage_error_and_sends_nothing() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&["new", "--context-json", r#"{"pwd":"/srv","tty":"/dev/pts/2"}"#, "--"]);

    // No daemon listens: the command must fail before it connects.
    let exit = run::run(&line, &ctx, &mut out).await;

    assert_eq!(exit, Exit::Usage);
    assert_eq!(captured.stdout(), "");
    assert!(captured.stderr().contains("efr new needs the first prompt"), "{}", captured.stderr());
    assert!(captured.stderr().contains("a bare ,new"), "{}", captured.stderr());
}

#[tokio::test]
async fn new_carries_the_turn_settings_of_the_flags_and_variables() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let vars = [(Var::Mode, "auto"), (Var::Model, "gpt-5.4"), (Var::Prompt, "start fresh")];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::PromptSend(params) = method else { panic!("expected prompt.send") };
        conn.reply(id, &sent()).await;
        let (sub, _) = conn.request().await;
        conn.item(sub, &item(6, Event::TurnCompleted { turn_id: turn(), usage: None })).await;
        conn.until_closed().await;
        params
    };
    let line = command(&["new", "--effort", "high"]);
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert!(params.new_conversation);
    assert_eq!(
        params.settings,
        TurnSettings {
            mode: Some(Mode::Auto),
            model: Some("gpt-5.4".to_owned()),
            effort: Some("high".to_owned()),
        }
    );
}
