use pretty_assertions::assert_eq;

use crate::run::os_name;

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

mod daemon {
    use std::path::PathBuf;

    use efr_protocol::{
        AdminConfigReload, AdminConfigReloadResult, AdminStatus, AdminStatusResult, CommandId,
        ConversationSubscribe, ConversationSubscribeItem, ConversationsList,
        ConversationsListResult, ErrorCode, Event, Method, ModelSource, ModelsList,
        ModelsListResult, PromptSend, PromptSendResult, PtyAttach, PtyId, PtyResize, Seq,
        ShellContext, Size, TurnSteer,
    };
    use efr_stdx::time::Clock as _;
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
            settings: efr_protocol::TurnSettings::default(),
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
                answers_input: false,
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

    /// Follows the turn of `sent` to its end, on a connection of its own.
    async fn follow_to_the_end(socket: &std::path::Path, sent: &PromptSendResult) {
        let (mut client, _) = RawClient::hello(socket, None).await;
        let stream = client
            .send(Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: sent.conversation_id,
                after_seq: Some(sent.seq),
                answers_input: false,
            }))
            .await;
        loop {
            let item = client.next(stream).await.unwrap().unwrap();
            let ConversationSubscribeItem::Event(envelope) = serde_json::from_value(item).unwrap()
            else {
                panic!("a resume right after the prompt replays events");
            };
            match envelope.event {
                Event::TurnCompleted { turn_id, .. } if turn_id == sent.turn_id => break,
                Event::TurnFailed { error, .. } => panic!("{error:?}"),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn new_settings_on_the_watch_reach_the_next_turn() {
        use std::sync::{Arc, Mutex};

        use crate::Settings;
        use crate::testing::{RecordingFactory, serve_with};

        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let deps =
            deps(&dirs, &clock).with_providers(Arc::new(RecordingFactory(Arc::clone(&requests))));
        let daemon = serve_with(Settings::default(), deps).await;
        let (mut client, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        let home = dirs.home().to_path_buf();

        let first: PromptSendResult =
            client.call(Method::PromptSend(prompt(1, "one", home.clone()))).await.unwrap();
        follow_to_the_end(&daemon.socket, &first).await;
        let mut changed = Settings::default();
        changed.model.name = Some("gpt-6-sol".to_owned());
        changed.model.system_prompt = "be brief".to_owned();
        changed.model.max_output_tokens = Some(512);
        daemon.settings.send_replace(Arc::new(changed));
        let second: PromptSendResult =
            client.call(Method::PromptSend(prompt(2, "two", home))).await.unwrap();
        follow_to_the_end(&daemon.socket, &second).await;

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].model, efr_provider_openai::DEFAULT_SUBSCRIPTION_MODEL);
        assert_eq!(requests[0].system.as_deref(), Some(crate::DEFAULT_SYSTEM_PROMPT));
        assert_eq!(requests[0].max_output_tokens, None);
        assert_eq!(requests[1].model, "gpt-6-sol");
        assert_eq!(requests[1].system.as_deref(), Some("be brief"));
        assert_eq!(requests[1].max_output_tokens, Some(512));

        drop(client);
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
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
                answers_input: false,
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
    async fn an_answer_with_nothing_to_answer_is_refused_without_repeating_it() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        let answer = |text: &str| {
            Method::InputRespond(efr_protocol::InputRespond {
                conversation_id: efr_protocol::ConversationId::from_uuid(uuid::Uuid::from_u128(4)),
                call_id: efr_protocol::CallId::from_uuid(uuid::Uuid::from_u128(5)),
                text: efr_protocol::SecretText::new(text),
                hidden: true,
                manual: false,
            })
        };

        let unknown = client.call::<serde_json::Value>(answer("hunter2")).await.unwrap_err();
        assert_eq!(unknown.code, ErrorCode::NotFound);
        assert!(!unknown.message.contains("hunter2"), "{}", unknown.message);
        let two_lines = client.call::<serde_json::Value>(answer("hunter2\rls")).await.unwrap_err();
        assert_eq!(two_lines.code, ErrorCode::Invalid);
        assert!(!two_lines.message.contains("hunter2"), "{}", two_lines.message);
        let long = "x".repeat(1025);
        let too_long = client.call::<serde_json::Value>(answer(&long)).await.unwrap_err();
        assert_eq!(too_long.code, ErrorCode::Invalid);

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
                answers_input: false,
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

    #[tokio::test]
    async fn a_terminal_that_followed_its_turn_to_the_end_gets_no_notice() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut terminal, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
        let sent: PromptSendResult = terminal
            .call(Method::PromptSend(prompt(1, "say hello", dirs.home().to_path_buf())))
            .await
            .unwrap();
        let stream = terminal
            .send(Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: sent.conversation_id,
                after_seq: Some(sent.seq),
                answers_input: false,
            }))
            .await;
        let mut completed = None;
        while let Some(item) = terminal.next(stream).await.unwrap() {
            if item["event"]["kind"] == "turn_completed" {
                completed = item["seq"].as_u64().map(Seq::new);
                break;
            }
        }
        let completed = completed.unwrap();
        // As efr does: it leaves as soon as it has shown the end of the turn, which
        // may be before the notices decide on that event.
        drop(terminal);
        // Here the notices task usually decides before the connection goes, so the
        // record that covers the other order is checked directly. No subscription was
        // handed the last possible event, so that one is attached only while the
        // request is open.
        let attached =
            |seq| daemon.connections.attached(TTY, sent.conversation_id, clock.now(), seq);
        let mut open = true;
        for _ in 0..100_000 {
            open = attached(Seq::new(u64::MAX));
            if !open {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!open, "the subscription ends with its connection");
        assert!(attached(completed), "the ended subscription was handed the turn's last event");

        // A turn in another terminal that nobody follows. Notices are decided in commit
        // order, so once its notice is there, the first terminal's turn was decided.
        let (mut other, _) = RawClient::hello(&daemon.socket, Some("/dev/pts/8")).await;
        let mut elsewhere = prompt(2, "say hello again", dirs.home().to_path_buf());
        if let Some(context) = elsewhere.context.as_mut() {
            context.tty = Some("/dev/pts/8".to_owned());
        }
        let _: PromptSendResult = other.call(Method::PromptSend(elsewhere)).await.unwrap();
        let file = dirs.dirs().runtime().join("notices/pts-8");
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
        assert_eq!(notice.as_deref(), Some("efr: turn finished: say hello again\n"));
        assert!(!dirs.dirs().runtime().join("notices/pts-7").exists());

        drop(other);
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
        let mut config = crate::Settings::default();
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
                answers_input: false,
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

    /// Runs one prompt whose model asks for `line` in a real hidden zsh in the home
    /// directory, which holds `notes.txt`, and answers every approval with allow. The
    /// answer: whether the call needed approval, and its output.
    async fn run_in_zsh(line: &str, config: crate::Settings) -> (bool, Option<String>) {
        use std::collections::BTreeMap;
        use std::sync::Arc;

        use efr_protocol::{ApprovalDecision, ApprovalRespond, ApprovalRespondResult};

        use crate::testing::{RunsOneCommandFactory, serve_with};

        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            std::fs::write(dirs.home().join(file), "").unwrap();
        }
        std::fs::write(dirs.home().join("notes.txt"), "hello\n").unwrap();
        let env = BTreeMap::from([
            ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ]);
        let mut deps = deps(&dirs, &clock)
            .with_shell_env(env)
            .with_providers(Arc::new(RunsOneCommandFactory(line.to_owned())));
        deps.holder = None;
        let mut config = config;
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
                answers_input: false,
            }))
            .await;
        let mut asked = false;
        let mut output = None;
        loop {
            let item = terminal.next(stream).await.unwrap().unwrap();
            let ConversationSubscribeItem::Event(envelope) = serde_json::from_value(item).unwrap()
            else {
                panic!("a resume right after the prompt replays events");
            };
            match envelope.event {
                Event::ApprovalRequested { call_id, .. } => {
                    asked = true;
                    let answer = Method::ApprovalRespond(ApprovalRespond {
                        command_id: command(2),
                        conversation_id: sent.conversation_id,
                        call_id,
                        decision: ApprovalDecision::Allow,
                    });
                    let _: ApprovalRespondResult = other.call(answer).await.unwrap();
                }
                Event::ToolCallCompleted { output: text, .. } => output = Some(text),
                Event::TurnCompleted { .. } => break,
                Event::TurnFailed { error, .. } => panic!("{error:?}"),
                _ => {}
            }
        }
        drop((terminal, other));
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
        (asked, output)
    }

    #[tokio::test]
    async fn e2e_a_read_only_command_runs_in_the_hidden_zsh_without_approval() {
        if !zsh_enabled("e2e_a_read_only_command_runs_in_the_hidden_zsh_without_approval") {
            return;
        }

        let (asked, output) = run_in_zsh("ls && cat notes.txt", crate::Settings::default()).await;

        assert!(!asked, "ls and cat run without approval");
        let output = output.unwrap_or_default();
        assert!(output.contains("notes.txt") && output.contains("hello"), "{output}");
    }

    #[tokio::test]
    async fn e2e_a_rule_in_the_config_lets_a_command_run_without_approval() {
        use efr_permissions::{Action, CommandPattern, Effect, Policy, Resource, Rule};

        if !zsh_enabled("e2e_a_rule_in_the_config_lets_a_command_run_without_approval") {
            return;
        }
        let mut config = crate::Settings::default();
        config.permissions.rules = Policy::new(vec![Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("seq").with_args(["3"])),
            Effect::Allow,
        )])
        .unwrap();

        let (asked, output) = run_in_zsh("seq 3", config.clone()).await;
        assert!(!asked, "the configured rule allows seq 3");
        assert!(output.unwrap_or_default().starts_with("1\n2\n3\n"));

        let (asked, _) = run_in_zsh("seq 4", config).await;
        assert!(asked, "seq 4 is not what the rule names");
    }

    #[tokio::test]
    async fn the_model_list_follows_the_latest_settings() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello(&daemon.socket, None).await;

        let before: ModelsListResult =
            client.call(Method::ModelsList(ModelsList::default())).await.unwrap();
        let mut changed = crate::Settings::default();
        changed.openai.models = Some(vec!["gpt-next".to_owned()]);
        changed.model.name = Some("gpt-6-sol".to_owned());
        daemon.settings.send_replace(std::sync::Arc::new(changed));
        let after: ModelsListResult =
            client.call(Method::ModelsList(ModelsList::default())).await.unwrap();

        let defaults = |list: &ModelsListResult| -> Vec<String> {
            list.models.iter().filter(|model| model.default).map(|model| model.id.clone()).collect()
        };
        assert_eq!(defaults(&before), ["gpt-5.5"]);
        assert!(before.models.iter().all(|model| model.source == ModelSource::Builtin));
        assert_eq!(defaults(&after), ["gpt-6-sol"]);
        let added = after.models.last().unwrap();
        assert_eq!((added.id.as_str(), added.source), ("gpt-next", ModelSource::Config));

        drop(client);
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn the_status_reports_the_roots_and_the_config_file_and_a_reload_answers() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello(&daemon.socket, None).await;

        let status: AdminStatusResult =
            client.call(Method::AdminStatus(AdminStatus::default())).await.unwrap();
        let roots = status.roots.unwrap();
        assert_eq!(roots.config.path, dirs.dirs().config());
        assert_eq!(roots.state.path, dirs.dirs().state());
        let config = status.config.unwrap();
        assert_eq!(config.path, dirs.dirs().config().join("config.toml"));
        assert!(!config.exists);
        let reloaded: AdminConfigReloadResult =
            client.call(Method::AdminConfigReload(AdminConfigReload::default())).await.unwrap();
        assert!(reloaded.applied);

        drop(client);
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_second_daemon_on_the_same_data_is_refused() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let daemon = serve(&dirs, &clock).await;

        let second = crate::start(crate::Settings::default(), deps(&dirs, &clock)).await;

        assert!(matches!(second, Err(DaemonError::AlreadyRunning { .. })), "{second:?}");
        daemon.shutdown.cancel();
        daemon.served.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_socket_path_too_long_for_a_socket_fails_the_start_with_the_fix() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let deep = dirs.root().join("x".repeat(120));
        let roots = efr_stdx::paths::Dirs::new(
            dirs.dirs().config(),
            dirs.dirs().data(),
            dirs.dirs().state(),
            &deep,
        )
        .unwrap();
        let mut deps = deps(&dirs, &clock);
        deps.dirs = roots;

        let error = crate::start(crate::Settings::default(), deps).await.unwrap_err();

        assert!(matches!(error, DaemonError::SocketPath { .. }), "{error:?}");
        assert_eq!(
            error.to_string(),
            "the socket path cannot be used; set EFR_RUNTIME_DIR to a shorter directory"
        );
        let cause = std::error::Error::source(&error).unwrap().to_string();
        assert!(cause.contains(&deep.join("daemon.sock").display().to_string()), "{cause}");
        assert!(!dirs.dirs().data().join("daemon.lock").exists(), "nothing else happened");
    }

    #[tokio::test]
    async fn a_runtime_root_whose_socket_path_just_fits_serves() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let fixed = dirs.root().as_os_str().len() + "/".len() + "/daemon.sock".len();
        let deep = dirs.root().join("r".repeat(efr_stdx::paths::MAX_SOCKET_PATH - fixed));
        let roots = efr_stdx::paths::Dirs::new(
            dirs.dirs().config(),
            dirs.dirs().data(),
            dirs.dirs().state(),
            &deep,
        )
        .unwrap();
        let mut deps = deps(&dirs, &clock);
        deps.dirs = roots;

        let daemon = crate::testing::serve_with(crate::Settings::default(), deps).await;

        assert_eq!(daemon.socket.as_os_str().len(), efr_stdx::paths::MAX_SOCKET_PATH);
        let (client, _) = RawClient::hello(&daemon.socket, None).await;
        drop(client);
        daemon.stop().await;
    }
}
