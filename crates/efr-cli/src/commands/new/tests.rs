use efr_protocol::{ClientFrame, Event, Method, PromptSendResult, Seq};
use pretty_assertions::assert_eq;

use crate::error::Exit;
use crate::run;
use crate::testing::{CONVERSATION, TestEnv, capture, command, conversation, item, turn};

fn sent() -> PromptSendResult {
    PromptSendResult {
        conversation_id: conversation(),
        turn_id: turn(),
        seq: Seq::new(5),
        queued: false,
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
async fn new_without_a_prompt_only_starts_the_conversation() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&["new", "--context-json", r#"{"pwd":"/srv","tty":"/dev/pts/2"}"#, "--"]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::PromptSend(params) = method else { panic!("expected prompt.send") };
        conn.reply(id, &sent()).await;
        // No subscription follows.
        let rest = conn.until_closed().await;
        assert!(rest.iter().all(|frame| !matches!(frame, ClientFrame::Request { .. })), "{rest:?}");
        params
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert!(params.new_conversation);
    assert_eq!(params.text, "");
    assert_eq!(params.context.unwrap().tty.as_deref(), Some("/dev/pts/2"));
    assert_eq!(captured.stdout(), "");
    assert_eq!(captured.stderr(), format!("new conversation {CONVERSATION}\n"));
}
