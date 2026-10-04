use std::path::PathBuf;
use std::time::Duration;

use efr_conversation::HostInfo;
use jiff::tz::TimeZone;
use pretty_assertions::assert_eq;

use crate::config::Config;
use efr_permissions::PathClass;

use crate::run::{conversation_config, engine, os_name};

#[test]
fn the_os_name_is_the_pretty_name_then_the_name() {
    let arch = "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n";
    let debian = "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nNAME=\"Debian GNU/Linux\"\n";
    let bare = "NAME=Alpine\n";

    assert_eq!(os_name(arch).as_deref(), Some("Arch Linux"));
    assert_eq!(os_name(debian).as_deref(), Some("Debian GNU/Linux 13 (trixie)"));
    assert_eq!(os_name(bare).as_deref(), Some("Alpine"));
    assert_eq!(os_name("ID=x\n"), None);
    assert_eq!(os_name("PRETTY_NAME=\"\"\n"), None);
}

#[tokio::test]
async fn the_secret_paths_of_the_config_classify_as_secrets() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let home = efr_scope::Home::new(&home).unwrap();
    let secret_paths = [PathBuf::from("~/.config/rclone/rclone.conf"), PathBuf::from("/srv/vault")];

    let engine = engine(
        &home,
        &home.path().join("secrets"),
        &secret_paths,
        &home.path().join("projects.toml"),
    )
    .await
    .unwrap();

    let scratch = home.path().join("scratch");
    let class = |path: PathBuf| engine.locations().classify(&path, &scratch);
    assert_eq!(class(home.path().join(".config/rclone/rclone.conf")), Some(PathClass::Secrets));
    assert_eq!(class(PathBuf::from("/srv/vault/token")), Some(PathClass::Secrets));
    assert_eq!(class(home.path().join(".config/rclone/other.conf")), Some(PathClass::UserConfig));
}

#[test]
fn the_conversation_settings_follow_the_config() {
    let mut config = Config::default();
    config.max_output_tokens = Some(2048);
    config.conversation.max_queued = 3;
    config.conversation.approval_timeout_secs = Some(90);
    config.conversation.update_interval_ms = 50;
    config.system_prompt = "be brief".to_owned();
    let host = HostInfo::new(Some("box".to_owned()), Some("Arch Linux".to_owned()));

    let settings = conversation_config(
        &config,
        "gpt-6-sol",
        PathBuf::from("/d/scratch"),
        host.clone(),
        TimeZone::UTC,
    );

    assert_eq!(settings.model, "gpt-6-sol");
    assert_eq!(settings.scratch_root, PathBuf::from("/d/scratch"));
    assert_eq!(settings.system_prompt.as_deref(), Some("be brief"));
    assert_eq!(settings.max_output_tokens, Some(2048));
    assert_eq!(settings.max_queued, 3);
    assert_eq!(settings.approval_timeout, Some(Duration::from_secs(90)));
    assert_eq!(settings.update_interval, Duration::from_millis(50));
    assert_eq!(settings.host, host);
}

mod daemon {
    use std::path::PathBuf;

    use efr_protocol::{
        AdminStatus, AdminStatusResult, CommandId, ConversationSubscribe,
        ConversationSubscribeItem, ConversationsList, ConversationsListResult, ErrorCode, Event,
        Method, PromptSend, PromptSendResult, PtyAttach, PtyId, PtyResize, ShellContext, Size,
        TurnSteer,
    };
    use efr_test_support::{TestClock, TestDirs};
    use pretty_assertions::assert_eq;

    use crate::DaemonError;
    use crate::testing::{ANSWER, RawClient, deps, serve};

    const TTY: &str = "/dev/pts/7";

    fn command(n: u128) -> CommandId {
        CommandId::from_uuid(uuid::Uuid::from_u128(n))
    }

    fn prompt(n: u128, text: &str, cwd: PathBuf) -> PromptSend {
        let mut context = ShellContext::new(cwd);
        context.tty = Some(TTY.to_owned());
        PromptSend {
            command_id: command(n),
            conversation_id: None,
            new_conversation: false,
            text: text.to_owned(),
            context: Some(context),
            last_command: None,
        }
    }

    #[tokio::test]
    async fn a_prompt_is_answered_and_followed_to_the_end_of_its_turn() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut client, hello) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        assert_eq!(hello.capabilities.admin, Some(true));
        assert_eq!(hello.paths.data_dir, dirs.dirs().data());

        let sent: PromptSendResult = client
            .call(Method::PromptSend(prompt(1, "say hello", dirs.home().to_path_buf())))
            .await
            .unwrap();
        assert!(!sent.queued);
        let stream = client
            .send(Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: sent.conversation_id,
                after_seq: Some(sent.seq),
            }))
            .await;
        let mut answer = None;
        loop {
            let item = client.next(stream).await.unwrap().unwrap();
            let ConversationSubscribeItem::Event(envelope) = serde_json::from_value(item).unwrap()
            else {
                panic!("a resume right after the prompt replays events");
            };
            assert!(envelope.seq > sent.seq);
            match envelope.event {
                Event::AssistantMessageCompleted { text, .. } => answer = Some(text),
                Event::TurnCompleted { turn_id, .. } => {
                    assert_eq!(turn_id, sent.turn_id);
                    break;
                }
                Event::TurnFailed { error, .. } => panic!("{error:?}"),
                _ => {}
            }
        }
        assert_eq!(answer.as_deref(), Some(ANSWER));

        let (mut other, _) = RawClient::hello(&daemon.socket, None).await;
        let list: ConversationsListResult =
            other.call(Method::ConversationsList(ConversationsList::default())).await.unwrap();
        assert_eq!(list.conversations.len(), 1);
        assert_eq!(list.conversations[0].id, sent.conversation_id);
        assert_eq!(list.conversations[0].tty.as_deref(), Some(TTY));
        assert_eq!(list.conversations[0].title.as_deref(), Some("say hello"));

        let status: AdminStatusResult =
            other.call(Method::AdminStatus(AdminStatus::default())).await.unwrap();
        assert_eq!(status.daemon_id, hello.daemon_id);
        assert_eq!(status.screen_backend, "vt100");
        assert_eq!(status.conversations, 1);

        drop((client, other));
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
        assert!(!dirs.dirs().daemon_json_path().exists());
        assert!(!daemon.socket.exists());
    }

    #[tokio::test]
    async fn a_second_prompt_from_the_terminal_continues_its_conversation_and_a_retry_runs_once() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        let home = dirs.home().to_path_buf();

        let first: PromptSendResult =
            client.call(Method::PromptSend(prompt(1, "one", home.clone()))).await.unwrap();
        let retried: PromptSendResult =
            client.call(Method::PromptSend(prompt(1, "one", home.clone()))).await.unwrap();
        let second: PromptSendResult =
            client.call(Method::PromptSend(prompt(2, "two", home.clone()))).await.unwrap();

        assert_eq!(retried, first, "a retry answers from the receipt");
        assert_eq!(second.conversation_id, first.conversation_id);
        assert_ne!(second.turn_id, first.turn_id);

        let new = PromptSend { new_conversation: true, ..prompt(3, "three", home) };
        let third: PromptSendResult = client.call(Method::PromptSend(new)).await.unwrap();
        assert_ne!(third.conversation_id, first.conversation_id, ",new starts another");

        drop(client);
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn refusals_are_coded_and_a_retried_refusal_stays_refused() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        let sent: PromptSendResult = client
            .call(Method::PromptSend(prompt(1, "hi", dirs.home().to_path_buf())))
            .await
            .unwrap();
        // Wait for the turn to end, so steering finds no running turn.
        let stream = client
            .send(Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: sent.conversation_id,
                after_seq: Some(sent.seq),
            }))
            .await;
        while let Some(item) = client.next(stream).await.unwrap() {
            if item["event"]["kind"] == "turn_completed" {
                break;
            }
        }

        let steer = Method::TurnSteer(TurnSteer {
            command_id: command(9),
            conversation_id: sent.conversation_id,
            turn_id: None,
            text: "faster".to_owned(),
        });
        let first = client.call::<serde_json::Value>(steer.clone()).await.unwrap_err();
        let again = client.call::<serde_json::Value>(steer).await.unwrap_err();
        assert_eq!(first.code, ErrorCode::Conflict);
        assert_eq!(again, first);

        let pty_id = PtyId::from_uuid(uuid::Uuid::from_u128(5));
        let attach =
            Method::PtyAttach(PtyAttach { pty_id, since_seq: None, scrollback_rows: None });
        let attach = client.send(attach).await;
        assert_eq!(client.next(attach).await.unwrap_err().code, ErrorCode::NotFound);
        let resize = Method::PtyResize(PtyResize { pty_id, size: Size { cols: 0, rows: 24 } });
        assert_eq!(
            client.call::<serde_json::Value>(resize).await.unwrap_err().code,
            ErrorCode::Invalid
        );

        drop(client);
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_finished_turn_leaves_a_notice_for_a_terminal_that_does_not_follow_it() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut terminal, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        let (mut watcher, _) = RawClient::hello(&daemon.socket, None).await;
        let sent: PromptSendResult = terminal
            .call(Method::PromptSend(prompt(1, "say hello", dirs.home().to_path_buf())))
            .await
            .unwrap();
        let stream = watcher
            .send(Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: sent.conversation_id,
                after_seq: Some(sent.seq),
            }))
            .await;
        while let Some(item) = watcher.next(stream).await.unwrap() {
            if item["event"]["kind"] == "turn_completed" {
                break;
            }
        }

        // The notice is written by its own task after the commit; yield until it is.
        let file = dirs.dirs().runtime().join("notices/pts-7");
        let mut notice = None;
        for _ in 0..100_000 {
            if let Ok(text) = std::fs::read_to_string(&file)
                && !text.is_empty()
            {
                notice = Some(text);
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(notice.as_deref(), Some("efr: turn finished: say hello\n"));

        drop((terminal, watcher));
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
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
    async fn e2e_an_approved_command_runs_in_the_hidden_zsh_and_its_pty_can_be_attached() {
        use std::collections::BTreeMap;
        use std::sync::Arc;

        use efr_protocol::{
            ApprovalDecision, ApprovalRespond, ApprovalRespondResult, Base64Bytes, PtyAttachItem,
            PtyWrite, PtyWriteResult,
        };

        use crate::testing::{RunsOneCommandFactory, serve_with};

        if !zsh_enabled(
            "e2e_an_approved_command_runs_in_the_hidden_zsh_and_its_pty_can_be_attached",
        ) {
            return;
        }
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            std::fs::write(dirs.home().join(file), "").unwrap();
        }
        let env = BTreeMap::from([
            ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ]);
        let mut deps = deps(&dirs, &clock)
            .with_shell_env(env)
            .with_providers(Arc::new(RunsOneCommandFactory("echo efr-e2e-$((40+2))".to_owned())));
        deps.holder = None;
        let mut config = crate::Config::default();
        config.shell.login = false;
        let daemon = serve_with(config, deps).await;
        let (mut terminal, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        let (mut other, _) = RawClient::hello(&daemon.socket, None).await;

        let sent: PromptSendResult = terminal
            .call(Method::PromptSend(prompt(1, "run it", dirs.home().to_path_buf())))
            .await
            .unwrap();
        let stream = terminal
            .send(Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: sent.conversation_id,
                after_seq: Some(sent.seq),
            }))
            .await;
        let mut pty = None;
        let mut output = None;
        loop {
            let item = terminal.next(stream).await.unwrap().unwrap();
            let ConversationSubscribeItem::Event(envelope) = serde_json::from_value(item).unwrap()
            else {
                panic!("a resume right after the prompt replays events");
            };
            match envelope.event {
                Event::ApprovalRequested { call_id, .. } => {
                    let answer = Method::ApprovalRespond(ApprovalRespond {
                        command_id: command(2),
                        conversation_id: sent.conversation_id,
                        call_id,
                        decision: ApprovalDecision::Allow,
                    });
                    let _: ApprovalRespondResult = other.call(answer).await.unwrap();
                }
                Event::ShellStarted { pty_id, .. } => pty = Some(pty_id),
                Event::ToolCallCompleted { output: text, .. } => output = Some(text),
                Event::TurnCompleted { .. } => break,
                Event::TurnFailed { error, .. } => panic!("{error:?}"),
                _ => {}
            }
        }
        assert!(output.as_deref().is_some_and(|text| text.contains("efr-e2e-42")), "{output:?}");
        let pty_id = pty.expect("the command started a shell");

        let attach = other
            .send(Method::PtyAttach(PtyAttach { pty_id, since_seq: None, scrollback_rows: None }))
            .await;
        let first: PtyAttachItem =
            serde_json::from_value(other.next(attach).await.unwrap().unwrap()).unwrap();
        assert!(matches!(first, PtyAttachItem::Snapshot { .. }), "{first:?}");
        let typed = Method::PtyWrite(PtyWrite {
            pty_id,
            data: Base64Bytes::new(b"echo attached-$((6*7))\r".to_vec()),
        });
        let (mut typist, _) = RawClient::hello(&daemon.socket, None).await;
        let _: PtyWriteResult = typist.call(typed).await.unwrap();
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("attached-42") {
            let item: PtyAttachItem =
                serde_json::from_value(other.next(attach).await.unwrap().unwrap()).unwrap();
            if let PtyAttachItem::Output { data, .. } = item {
                seen.extend_from_slice(data.as_bytes());
            }
        }

        drop((terminal, other, typist));
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_second_daemon_on_the_same_data_is_refused() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;

        let second = crate::start(crate::Config::default(), deps(&dirs, &clock)).await;

        assert!(matches!(second, Err(DaemonError::AlreadyRunning { .. })), "{second:?}");
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
    }
}
