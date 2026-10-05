//! The shell tool over a `TestDaemon`: the model's command runs in the conversation's
//! hidden shell, played by the fake PTY holder in the scenarios and by a real zsh in
//! the `shell_` test.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, Event, Method, PromptSendResult,
    PtyAttach, PtyAttachItem,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{Replay, TTY, TestDaemon, command_id, events_until};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

/// The `tool_call_completed` events of `replay`'s conversation: exit code, error flag
/// and output.
async fn completed_calls(replay: &Replay) -> Vec<(Option<i32>, bool, String)> {
    replay
        .events()
        .await
        .unwrap()
        .into_iter()
        .filter_map(|envelope| match envelope.event {
            Event::ToolCallCompleted { exit_code, is_error, output, .. } => {
                Some((exit_code, is_error, output))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn tool_call_shell_ok() {
    let mut replay = Replay::run("tool_call_shell_ok").await.unwrap();

    let holder = replay.daemon().holder().unwrap().clone();
    let [spec] = holder.specs().try_into().unwrap();
    assert_eq!(spec.cwd, replay.daemon().cwd(), "the shell starts where the user is");
    assert!(spec.program.ends_with("zsh"));
    assert_eq!(spec.env.get("EFR_HIDDEN_SHELL").map(String::as_str), Some("1"));
    let terminal = replay.terminal("shell").await.unwrap().pty_id();
    assert_eq!(replay.bindings().get("<pty:1>"), Some(terminal.to_string().as_str()));
    let calls = completed_calls(&replay).await;
    let [(exit_code, is_error, output)] = calls.as_slice() else { panic!("{calls:?}") };
    assert_eq!(*exit_code, Some(0));
    assert!(!is_error);
    assert!(output.starts_with("notes.txt\nsrc\n"), "{output}");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn tool_call_shell_nonzero_exit() {
    let replay = Replay::run("tool_call_shell_nonzero_exit").await.unwrap();

    let calls = completed_calls(&replay).await;
    let [(exit_code, _, output)] = calls.as_slice() else { panic!("{calls:?}") };
    assert_eq!(*exit_code, Some(2), "the model sees the command's own status");
    assert!(output.contains("No such file or directory"), "{output}");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn the_shell_stays_for_the_next_command_of_the_conversation() {
    let mut replay = Replay::run("tool_call_shell_ok").await.unwrap();

    // The shell that ran the scenario's command is still at its prompt, so a write
    // reaches it and nothing spawns a second one.
    let pty_id = replay.terminal("shell").await.unwrap().pty_id();
    let typed = Method::PtyWrite(efr_protocol::PtyWrite {
        pty_id,
        data: efr_protocol::Base64Bytes::new(b"echo again\r".to_vec()),
    });
    let _: efr_protocol::PtyWriteResult = replay.client().call(typed).await.unwrap();
    let terminal = replay.terminal("shell").await.unwrap();
    assert_eq!(terminal.typed_line().await.unwrap(), b"echo again\r");
    terminal.run(b"again\r\n", 0).await.unwrap();
    assert_eq!(replay.daemon().holder().unwrap().spawned(), 1);
    replay.stop().await.unwrap();
}

/// A model that asks for one shell command, then says `done` once the result is back.
#[derive(Debug)]
struct RunsOneCommand {
    id: ProviderId,
    command: String,
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for RunsOneCommand {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let arguments = serde_json::json!({ "command": self.command }).to_string();
            vec![
                ProviderEvent::ToolCallStart {
                    call_id: "call_1".to_owned(),
                    name: "shell".to_owned(),
                },
                ProviderEvent::ToolCallEnd { call_id: "call_1".to_owned(), arguments },
                ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

#[expect(clippy::print_stderr, reason = "a skipped test says why, as atuin's e2e tests do")]
fn zsh_enabled(test: &str) -> bool {
    let on = efr_stdx::env::flag(efr_stdx::env::Var::TestZsh).unwrap_or(false);
    if !on {
        eprintln!("skipping {test}: set EFR_TEST_ZSH=1 to run the tests that drive a real zsh");
    }
    on
}

#[tokio::test]
async fn shell_a_command_runs_in_a_real_zsh_and_its_output_is_recorded() {
    if !zsh_enabled("shell_a_command_runs_in_a_real_zsh_and_its_output_is_recorded") {
        return;
    }
    let provider = RunsOneCommand {
        id: ProviderId::new("test").unwrap(),
        command: "cd .. && echo efr-$((40+2))".to_owned(),
        calls: AtomicUsize::new(0),
    };
    let daemon = TestDaemon::builder()
        .local_pty()
        .custom_provider(Arc::new(provider))
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let sent: PromptSendResult = client.call(daemon.prompt(1, "run it", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(event, Event::ApprovalRequested { .. } | Event::TurnCompleted { .. })
                || matches!(event, Event::TurnFailed { .. })
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events);
        match last {
            Event::ApprovalRequested { call_id, .. } => {
                let answer = Method::ApprovalRespond(ApprovalRespond {
                    command_id: command_id(2),
                    conversation_id: sent.conversation_id,
                    call_id,
                    decision: ApprovalDecision::Allow,
                });
                let _: ApprovalRespondResult = client.call(answer).await.unwrap();
            }
            Event::TurnFailed { error, .. } => panic!("{error:?}"),
            _ => break,
        }
    }

    let output = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, exit_code, .. } => Some((output.clone(), *exit_code)),
        _ => None,
    });
    let (output, exit_code) = output.unwrap();
    assert!(output.contains("efr-42"), "{output}");
    assert_eq!(exit_code, Some(0));
    let home = daemon.dirs().home().to_path_buf();
    assert!(
        seen.iter().any(|envelope| matches!(&envelope.event,
            Event::CwdChanged { cwd, .. } if *cwd == home)),
        "the hidden shell reported its move to the parent directory: {seen:#?}"
    );
    let pty_id = seen
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::ShellStarted { pty_id, .. } => Some(*pty_id),
            _ => None,
        })
        .unwrap();

    // The recording holds the shell's bytes, marks included, from offset 0.
    let attach = Method::PtyAttach(PtyAttach {
        pty_id,
        since_seq: Some(efr_protocol::Seq::ZERO),
        scrollback_rows: None,
    });
    let mut stream = client.stream::<PtyAttachItem>(attach).await.unwrap();
    let mut recorded = Vec::new();
    while !String::from_utf8_lossy(&recorded).contains("efr-42") {
        match stream.next().await.unwrap().unwrap() {
            PtyAttachItem::Output { seq, data } => {
                assert_eq!(seq.get(), recorded.len() as u64, "the replay is back to back");
                recorded.extend_from_slice(data.as_bytes());
            }
            other => panic!("a resume from offset 0 replays the recording: {other:?}"),
        }
    }
    assert!(recorded.windows(8).any(|window| window == b"\x1b]133;C\x07"));
    drop((stream, follow, client));
    daemon.stop().await.unwrap();
}
