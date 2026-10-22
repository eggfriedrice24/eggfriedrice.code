use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    ConversationHistoryResult, ConversationStatus, ConversationSummary, ConversationsListResult,
    DesktopEnv, EffectiveSettings, ErrorBody, ErrorCode, Event, Method, Mode, Origin,
    OverriddenSettings, PageCursor, PromptSendResult, RecentCommand, Seq, TurnSettings,
    TurnSteerResult,
};
use efr_stdx::env::{Env, Var};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;

use super::overrides;
use crate::context::{Context, desktop_env};
use crate::error::Exit;
use crate::run;
use crate::terminal::Size;
use crate::testing::{
    CONVERSATION, Conn, FixedScreen, ScriptedKeys, TestEnv, bare, capture, command, conversation,
    item, now, readable, terminal_facts, turn,
};

const CONTEXT: &str = r#"{"pwd":"/etc/nginx","oldpwd":"/home/user","tty":"/dev/pts/3","shell_pid":4100,"last_status":1,"shlvl":1,"ssh_connection":null,"hostname":"box"}"#;

fn sent(queued: bool) -> PromptSendResult {
    PromptSendResult {
        conversation_id: conversation(),
        turn_id: turn(),
        seq: Seq::new(10),
        queued,
        settings: None,
        route: None,
    }
}

/// Answers the prompt with `result` and the turn with `reply`, then waits for the
/// client to leave. Returns the prompt's params.
async fn answer(
    conn: &mut Conn,
    result: PromptSendResult,
    reply: &str,
) -> efr_protocol::PromptSend {
    let (id, method) = conn.request().await;
    let Method::PromptSend(params) = method else {
        panic!("expected prompt.send, got {}", method.name());
    };
    conn.reply(id, &result).await;
    let (sub, method) = conn.request().await;
    assert!(matches!(method, Method::ConversationSubscribe(_)));
    let message =
        Event::AssistantMessageCompleted { turn_id: turn(), index: 0, text: reply.to_owned() };
    conn.item(sub, &item(11, message)).await;
    conn.item(
        sub,
        &item(
            12,
            Event::TurnCompleted {
                turn_id: turn(),
                usage: None,
                changes: None,
                context: None,
                stop: None,
                undo: None,
            },
        ),
    )
    .await;
    conn.until_closed().await;
    params
}

#[tokio::test]
async fn send_relays_the_plugins_context_and_last_command_and_writes_the_raw_reply() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&[
        "send",
        "--context-json",
        CONTEXT,
        "--last-command",
        "nginx -t",
        "--",
        "why",
        "does",
        "it",
        "fail?",
    ]);
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Shell);
        assert_eq!(conn.hello().tty.as_deref(), Some("/dev/pts/3"));
        assert_eq!(
            conn.hello().client.as_deref(),
            Some(concat!("efr ", env!("CARGO_PKG_VERSION")))
        );
        answer(&mut conn, sent(false), "A **typo** on line 3.").await
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(params.text, "why does it fail?");
    assert_eq!(params.last_command.as_deref(), Some("nginx -t"));
    assert!(!params.new_conversation);
    assert_eq!(params.conversation_id, None);
    let context = params.context.unwrap();
    assert_eq!(context.pwd, PathBuf::from("/etc/nginx"));
    assert_eq!(context.last_status, Some(1));
    assert_eq!(context.hostname.as_deref(), Some("box"));
    assert_eq!(captured.stdout(), "A **typo** on line 3.\n");
    assert_eq!(captured.stderr(), "");
}

#[tokio::test]
async fn shell_context_carries_desktop_names_only() {
    // The shell's environment, with sockets and a cookie that must never leave it.
    let shell: std::collections::HashMap<&str, &str> = [
        ("HYPRLAND_INSTANCE_SIGNATURE", "abc_1760000000_1"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("DISPLAY", ""),
        ("XAUTHORITY", "/run/user/1000/xauth_x"),
        ("SSH_AUTH_SOCK", "/run/user/1000/ssh"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ("HYPRLAND_CMD", "Hyprland"),
    ]
    .into_iter()
    .collect();
    let desktop = desktop_env(|name| shell.get(name).map(|value| (*value).to_owned()));
    let want = DesktopEnv {
        hyprland_instance_signature: Some("abc_1760000000_1".to_owned()),
        wayland_display: Some("wayland-1".to_owned()),
        has_xauthority: true,
        ..DesktopEnv::default()
    };
    assert_eq!(desktop.as_deref(), Some(&want));
    assert_eq!(desktop_env(|_| None), None, "a console has none");
    let cookie_only = |name: &str| (name == "XAUTHORITY").then(|| "/home/u/.Xauthority".to_owned());
    assert_eq!(desktop_env(cookie_only), None, "a cookie alone names no desktop");

    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = Context { desktop, ..env.context() };
    let (mut out, _captured) = capture();
    // A context that claims other desktop variables gets the shell's.
    let forged = CONTEXT.replace('}', r#","desktop":{"swaysock":"/home/u/evil.sock"}}"#);
    let line = command(&["send", "--context-json", &forged, "--", "show", "my", "monitors"]);
    let script = async {
        let mut conn = daemon.accept().await;
        answer(&mut conn, sent(false), "Two monitors.").await
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let context = params.context.unwrap();
    assert_eq!(context.desktop.as_deref(), Some(&want));
    let wire = serde_json::to_value(&context).unwrap();
    assert_eq!(
        wire["desktop"],
        serde_json::json!({
            "wayland_display": "wayland-1",
            "hyprland_instance_signature": "abc_1760000000_1",
            "has_xauthority": true,
        })
    );
    assert!(!wire.to_string().contains("xauth_x"), "never the cookie's path: {wire}");
}

/// A context whose environment holds what the zsh plugin hands over.
fn plugin_context(env: &TestEnv, prompt: &str) -> Context {
    let vars = [(Var::Context, CONTEXT), (Var::LastCommand, "nginx -t"), (Var::Prompt, prompt)];
    Context { env: Env::fixed(vars), ..env.context() }
}

#[tokio::test]
async fn send_reads_what_the_plugin_hands_over_in_the_environment() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = plugin_context(&env, "why does it fail?");
    let (mut out, captured) = capture();
    let line = command(&["send"]);
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Shell);
        assert_eq!(conn.hello().tty.as_deref(), Some("/dev/pts/3"));
        answer(&mut conn, sent(false), "A typo.").await
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    assert_eq!(params.text, "why does it fail?");
    assert_eq!(params.last_command.as_deref(), Some("nginx -t"));
    let context = params.context.unwrap();
    assert_eq!(context.pwd, PathBuf::from("/etc/nginx"));
    assert_eq!(context.last_status, Some(1));
    assert_eq!(captured.stdout(), "A typo.\n");
}

#[tokio::test]
async fn send_relays_the_recent_commands_of_the_plugin() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let vars = [
        (Var::Context, CONTEXT),
        (Var::RecentCommands, r#"[{"command":"git pull","status":0},{"command":"nginx -t"}]"#),
        (Var::Prompt, "why"),
    ];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        answer(&mut conn, sent(false), "ok").await
    };
    let line = command(&["send"]);
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        params.recent_commands,
        [
            RecentCommand { status: Some(0), ..RecentCommand::new("git pull") },
            RecentCommand::new("nginx -t"),
        ]
    );
    let wire = serde_json::to_string(&params.context).unwrap();
    assert!(!wire.contains("git pull"), "never in the context: {wire}");
}

#[tokio::test]
async fn recent_commands_that_do_not_parse_still_send_the_prompt() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let vars = [(Var::Context, CONTEXT), (Var::RecentCommands, "[{"), (Var::Prompt, "why")];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        answer(&mut conn, sent(false), "ok").await
    };
    let line = command(&["send"]);
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert!(params.recent_commands.is_empty());
}

#[tokio::test]
async fn arguments_win_over_the_plugins_variables() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = plugin_context(&env, "from the variable");
    let (mut out, _captured) = capture();
    let line = command(&[
        "send",
        "--context-json",
        r#"{"pwd":"/srv"}"#,
        "--last-command",
        "ls",
        "--",
        "from",
        "the",
        "arguments",
    ]);
    let script = async {
        let mut conn = daemon.accept().await;
        answer(&mut conn, sent(false), "ok").await
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(params.text, "from the arguments");
    assert_eq!(params.last_command.as_deref(), Some("ls"));
    assert_eq!(params.context.unwrap().pwd, PathBuf::from("/srv"));
}

#[tokio::test]
async fn a_bad_context_variable_is_a_usage_error_that_names_it() {
    let env = TestEnv::new();
    let ctx =
        Context { env: Env::fixed([(Var::Context, "{pwd"), (Var::Prompt, "x")]), ..env.context() };
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["send"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Usage);
    assert!(captured.stderr().starts_with("efr: EFR_CONTEXT is not a shell context object: "));
}

#[tokio::test]
async fn an_empty_prompt_variable_is_an_empty_prompt() {
    let env = TestEnv::new();
    let ctx = Context { env: Env::fixed([(Var::Context, CONTEXT)]), ..env.context() };
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["send"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Usage);
    assert_eq!(captured.stderr(), "efr: the prompt is empty\n");
}

#[tokio::test]
async fn a_last_command_inside_the_context_json_is_dropped() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, _captured) = capture();
    let line = command(&[
        "send",
        "--context-json",
        r#"{"pwd":"/x","last_command":"export TOKEN=s3cret"}"#,
        "--",
        "hi",
    ]);
    let script = async {
        let mut conn = daemon.accept().await;
        answer(&mut conn, sent(false), "ok").await
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(params.last_command, None);
    let wire = serde_json::to_string(&params.context).unwrap();
    assert!(!wire.contains("s3cret"), "{wire}");
}

#[tokio::test]
async fn send_typed_by_hand_uses_the_working_directory_and_the_terminal() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = Context { tty: Some("/dev/pts/9".to_owned()), ..env.context() };
    let (mut out, _captured) = capture();
    let line = command(&["send", "--", "hello"]);
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Cli);
        assert_eq!(conn.hello().tty.as_deref(), Some("/dev/pts/9"));
        answer(&mut conn, sent(false), "hi").await
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let context = params.context.unwrap();
    assert_eq!(context.pwd, PathBuf::from("/home/user/project"));
    assert_eq!(context.tty.as_deref(), Some("/dev/pts/9"));
}

#[tokio::test]
async fn a_queued_prompt_says_so_and_still_follows_its_turn() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&["send", "--", "next"]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::PromptSend(_)), "{}", method.name());
        conn.reply(id, &sent(true)).await;
        // A queued prompt looks for the approvals that the running turn waits for.
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationHistory(_)), "{}", method.name());
        conn.reply(id, &ConversationHistoryResult::default()).await;
        let (sub, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationSubscribe(_)));
        let message = Event::AssistantMessageCompleted {
            turn_id: turn(),
            index: 0,
            text: "Later.".to_owned(),
        };
        conn.item(sub, &item(11, message)).await;
        conn.item(
            sub,
            &item(
                12,
                Event::TurnCompleted {
                    turn_id: turn(),
                    usage: None,
                    changes: None,
                    context: None,
                    stop: None,
                    undo: None,
                },
            ),
        )
        .await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(captured.stderr(), "queued behind the running turn\n");
    assert_eq!(captured.stdout(), "Later.\n");
}

#[tokio::test]
async fn on_a_terminal_the_reply_is_rendered() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = Context {
        term: terminal_facts(),
        screen: Arc::new(FixedScreen(Size { cols: 40, rows: 12 })),
        ..env.context()
    };
    let (mut out, captured) = capture();
    let seen = captured.clone();
    let line = command(&["send", "--", "plan?"]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &sent(false)).await;
        let (sub, _) = conn.request().await;
        let updates = ["# Plan\n\nRestart", "# Plan\n\nRestart nginx, then check `journalctl`."];
        let mut offset = 0;
        for (seq, text) in (11..).zip(updates) {
            let event = Event::AssistantMessageUpdated {
                turn_id: turn(),
                index: 0,
                offset: offset as u64,
                delta: text[offset..].to_owned(),
            };
            offset = text.len();
            conn.item(sub, &item(seq, event)).await;
            // Each update gets a frame of its own before the next one comes.
            let frames = usize::try_from(seq - 9).unwrap();
            Wait::new(&format!("frame {frames}"))
                .until(|| seen.stdout().matches("\x1b[?2026h").count() == frames)
                .await
                .unwrap();
        }
        let done = Event::AssistantMessageCompleted {
            turn_id: turn(),
            index: 0,
            text: format!("{}\n", updates[1]),
        };
        conn.item(sub, &item(13, done)).await;
        Wait::new("frame 4")
            .until(|| seen.stdout().matches("\x1b[?2026h").count() == 4)
            .await
            .unwrap();
        conn.item(
            sub,
            &item(
                14,
                Event::TurnCompleted {
                    turn_id: turn(),
                    usage: None,
                    changes: None,
                    context: None,
                    stop: None,
                    undo: None,
                },
            ),
        )
        .await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    insta::assert_snapshot!(readable(&captured.stdout()));
}

#[tokio::test]
async fn bad_context_json_is_a_usage_error_before_any_connection() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let exit =
        run::run(&command(&["send", "--context-json", "{pwd", "--", "x"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Usage);
    assert!(captured.stderr().starts_with("efr: --context-json is not a shell context object: "));
}

#[tokio::test]
async fn an_empty_prompt_is_a_usage_error() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["send", "--", " "]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Usage);
    assert_eq!(captured.stderr(), "efr: the prompt is empty\n");
}

#[tokio::test]
async fn without_a_daemon_send_exits_with_three_and_a_hint() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["send", "--", "hi"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::NotRunning);
    let socket = env.socket();
    assert_eq!(
        captured.stderr(),
        format!(
            "efr: no daemon is listening on {}\nefr: start the daemon with: systemctl --user start efrd, or `just run` in the efr checkout for a foreground one\n",
            socket.display()
        )
    );
}

#[tokio::test]
async fn a_refused_prompt_exits_with_one_and_the_daemons_message() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.fail(id, ErrorBody::new(ErrorCode::Busy, "the daemon is starting")).await;
        conn.until_closed().await;
    };
    let line = command(&["send", "--", "hi"]);
    let (exit, _) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(
        captured.stderr(),
        "efr: the daemon failed the request with busy: the daemon is starting\n"
    );
}

#[tokio::test]
async fn a_turn_without_a_login_says_how_to_log_in() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &sent(false)).await;
        let (sub, _) = conn.request().await;
        let error =
            ErrorBody::new(ErrorCode::Unauthorized, "no credentials are stored for the provider");
        conn.item(
            sub,
            &item(
                11,
                Event::TurnFailed {
                    turn_id: turn(),
                    error,
                    usage: None,
                    context: None,
                    changes: None,
                    undo: None,
                },
            ),
        )
        .await;
        conn.until_closed().await;
    };
    let line = command(&["send", "--", "hello"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(
        captured.stderr(),
        "efr: the turn failed with unauthorized: no credentials are stored for the provider\n\
         efr: log in with: efr login openai\n"
    );
}

fn summary(id: &str, tty: Option<&str>) -> ConversationSummary {
    ConversationSummary {
        id: id.parse().unwrap(),
        title: None,
        status: ConversationStatus::Running,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(1),
        cwd: None,
        scope: None,
        tty: tty.map(str::to_owned),
    }
}

#[tokio::test]
async fn steer_finds_the_terminals_conversation_across_pages() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line =
        command(&["send", "--steer", "--context-json", CONTEXT, "--", "use", "port", "8080"]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationsList(ref p) if p.cursor.is_none()));
        let first = ConversationsListResult {
            conversations: vec![summary(
                "019a9b1c-3d00-7a10-8b20-0000000000aa",
                Some("/dev/pts/1"),
            )],
            next_cursor: Some(PageCursor::new("p2")),
        };
        conn.reply(id, &first).await;
        let (id, method) = conn.request().await;
        assert!(
            matches!(method, Method::ConversationsList(ref p) if p.cursor.as_ref().map(PageCursor::as_str) == Some("p2"))
        );
        let second = ConversationsListResult {
            conversations: vec![summary(CONVERSATION, Some("/dev/pts/3"))],
            next_cursor: None,
        };
        conn.reply(id, &second).await;
        let (id, method) = conn.request().await;
        let Method::TurnSteer(params) = method else { panic!("expected turn.steer") };
        conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(30), queued: false })
            .await;
        conn.until_closed().await;
        params
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(params.conversation_id, conversation());
    assert_eq!(params.text, "use port 8080");
    assert_eq!(params.turn_id, None);
    assert_eq!(captured.stderr(), "steered the running turn\n");
}

#[tokio::test]
async fn steer_takes_its_text_and_terminal_from_the_plugins_variables() {
    let env = TestEnv::new();
    let daemon = env.listen();
    // The plugin sets no last command for a steer; one left in the environment is
    // ignored rather than refused.
    let ctx = plugin_context(&env, "use port 8080");
    let (mut out, _captured) = capture();
    let line = command(&["send", "--steer"]);
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().tty.as_deref(), Some("/dev/pts/3"));
        let (id, _) = conn.request().await;
        let list = ConversationsListResult {
            conversations: vec![summary(CONVERSATION, Some("/dev/pts/3"))],
            next_cursor: None,
        };
        conn.reply(id, &list).await;
        let (id, method) = conn.request().await;
        let Method::TurnSteer(params) = method else { panic!("expected turn.steer") };
        conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(30), queued: false })
            .await;
        conn.until_closed().await;
        params
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(params.text, "use port 8080");
}

#[tokio::test]
async fn steer_without_an_active_conversation_fails() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&["send", "--steer", "--context-json", CONTEXT, "--", "x"]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &ConversationsListResult::default()).await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(captured.stderr(), "efr: no conversation is active in /dev/pts/3\n");
}

#[tokio::test]
async fn steer_with_a_conversation_needs_no_lookup() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, _captured) = capture();
    let line = command(&["send", "--steer", "--conversation", CONVERSATION, "--", "x"]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::TurnSteer(ref p) if p.conversation_id == conversation()));
        conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(30), queued: false })
            .await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
}

#[tokio::test]
async fn steer_without_a_tty_or_a_conversation_is_a_usage_error() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, _captured) = capture();
    let exit = run::run(&command(&["send", "--steer", "--", "x"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Usage);
}

#[tokio::test]
async fn the_turn_settings_come_from_the_flags_over_the_variables() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let vars = [(Var::Mode, "manual"), (Var::Model, "gpt-5.5"), (Var::Effort, "low")];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, _captured) = capture();
    let line = command(&["send", "--mode", "auto", "--effort", "high", "--", "hi"]);
    let script = async {
        let mut conn = daemon.accept().await;
        answer(&mut conn, sent(false), "ok").await
    };
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        params.settings,
        TurnSettings {
            mode: Some(Mode::Auto),
            model: Some("gpt-5.5".to_owned()),
            effort: Some("high".to_owned()),
        }
    );
}

#[tokio::test]
async fn without_flags_or_variables_the_prompt_asks_for_no_settings() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        answer(&mut conn, sent(false), "ok").await
    };
    let line = command(&["send", "--", "hi"]);
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(params.settings, TurnSettings::default());
}

#[tokio::test]
async fn an_unknown_mode_variable_sends_nothing() {
    let env = TestEnv::new();
    let ctx = Context { env: Env::fixed([(Var::Mode, "fast")]), ..env.context() };
    let (mut out, captured) = capture();
    // No daemon listens: the command must fail before it connects.
    let exit = run::run(&command(&["send", "--", "hi"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Usage);
    let stderr = captured.stderr();
    assert!(stderr.starts_with("efr: EFR_MODE names the mode \"fast\""), "{stderr}");
}

#[test]
fn a_steer_takes_no_settings_flags() {
    for flag in ["--mode=auto", "--model=gpt-5.5", "--effort=high"] {
        let argv = ["efr", "send", "--steer", flag, "--", "x"];
        let error = <crate::cli::Cli as clap::Parser>::try_parse_from(argv).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict, "{flag}");
    }
}

#[tokio::test]
async fn a_steer_ignores_the_settings_variables() {
    let env = TestEnv::new();
    let daemon = env.listen();
    // A running turn keeps its settings, so a terminal's choice does not matter here,
    // not even an invalid one.
    let vars = [(Var::Context, CONTEXT), (Var::Prompt, "faster"), (Var::Mode, "fast")];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, _captured) = capture();
    let line = command(&["send", "--steer", "--conversation", CONVERSATION]);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::TurnSteer(_)), "{}", method.name());
        conn.reply(id, &TurnSteerResult { turn_id: turn(), seq: Seq::new(30), queued: false })
            .await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
}

fn effective(overridden: OverriddenSettings) -> EffectiveSettings {
    EffectiveSettings {
        mode: Mode::Auto,
        model: "gpt-5.4".to_owned(),
        effort: Some("high".to_owned()),
        overridden,
        fallback: None,
    }
}

#[test]
fn the_note_names_only_the_overridden_values() {
    assert_eq!(overrides(&effective(OverriddenSettings::default())), None);
    let all = OverriddenSettings { mode: true, model: true, effort: true };
    assert_eq!(
        overrides(&effective(all)).as_deref(),
        Some("mode auto, model gpt-5.4, effort high")
    );
    let model = OverriddenSettings { model: true, ..OverriddenSettings::default() };
    assert_eq!(overrides(&effective(model)).as_deref(), Some("model gpt-5.4"));
    let effort = OverriddenSettings { effort: true, ..OverriddenSettings::default() };
    let none_sent = EffectiveSettings { effort: None, ..effective(effort) };
    assert_eq!(overrides(&none_sent).as_deref(), Some("effort default"));
}

#[tokio::test]
async fn overridden_settings_are_the_first_line_of_the_reply() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&["send", "--mode", "auto", "--model", "gpt-5.4", "--", "next"]);
    let overridden =
        OverriddenSettings { mode: true, model: true, ..OverriddenSettings::default() };
    let result = PromptSendResult { settings: Some(effective(overridden)), ..sent(true) };
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &result).await;
        // Nothing arrives before the notes: they come from the prompt.send result.
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationHistory(_)), "{}", method.name());
        conn.reply(id, &ConversationHistoryResult::default()).await;
        let (sub, _) = conn.request().await;
        conn.item(
            sub,
            &item(
                12,
                Event::TurnCompleted {
                    turn_id: turn(),
                    usage: None,
                    changes: None,
                    context: None,
                    stop: None,
                    undo: None,
                },
            ),
        )
        .await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(captured.stderr(), "mode auto, model gpt-5.4\nqueued behind the running turn\n");
}

#[tokio::test]
async fn the_route_and_the_settings_share_the_first_line_of_the_reply() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&["send", "--mode", "auto", "--", "next"]);
    let overridden = OverriddenSettings { mode: true, ..OverriddenSettings::default() };
    let route = efr_protocol::Routed {
        scope: efr_protocol::RouteLabel {
            scope: efr_protocol::RouteScope::Outside,
            name: None,
            dir: "/etc/nixos".into(),
        },
        switch: Some(efr_protocol::Switch::New {
            from: None,
            reason: efr_protocol::NewReason::Idle { hours: 13 },
        }),
        hint: None,
    };
    let result = PromptSendResult {
        settings: Some(effective(overridden)),
        route: Some(Box::new(route)),
        ..sent(false)
    };
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &result).await;
        let (sub, _) = conn.request().await;
        let done = Event::TurnCompleted {
            turn_id: turn(),
            usage: None,
            changes: None,
            context: None,
            stop: None,
            undo: None,
        };
        conn.item(sub, &item(12, done)).await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        captured.stderr(),
        "↪ new conversation outside projects (the last one was idle for 13 h) · mode auto\n"
    );
}

#[tokio::test]
async fn the_repo_hint_follows_the_head_line() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let line = command(&["send", "--", "next"]);
    let route = efr_protocol::Routed {
        scope: efr_protocol::RouteLabel {
            scope: efr_protocol::RouteScope::Outside,
            name: None,
            dir: "/srv/tool/src".into(),
        },
        switch: None,
        hint: Some(efr_protocol::RepoHint { root: "/srv/tool".into() }),
    };
    let result = PromptSendResult { route: Some(Box::new(route)), ..sent(false) };
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &result).await;
        let (sub, _) = conn.request().await;
        let done = Event::TurnCompleted {
            turn_id: turn(),
            usage: None,
            changes: None,
            context: None,
            stop: None,
            undo: None,
        };
        conn.item(sub, &item(12, done)).await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        captured.stderr(),
        "○ no project · /srv/tool/src\n○ /srv/tool is a git repo but not a project: writes \
         there ask you. efr project add /srv/tool registers it.\n"
    );
}

#[tokio::test]
async fn on_a_terminal_the_settings_note_is_dim() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = Context {
        term: terminal_facts(),
        screen: Arc::new(FixedScreen(Size { cols: 40, rows: 12 })),
        ..env.context()
    };
    let (mut out, captured) = capture();
    let seen = captured.clone();
    let line = command(&["send", "--effort", "high", "--", "plan?"]);
    let overridden = OverriddenSettings { effort: true, ..OverriddenSettings::default() };
    let result = PromptSendResult { settings: Some(effective(overridden)), ..sent(false) };
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &result).await;
        let (sub, _) = conn.request().await;
        let done = Event::AssistantMessageCompleted {
            turn_id: turn(),
            index: 0,
            text: "Restart nginx.".to_owned(),
        };
        conn.item(sub, &item(11, done)).await;
        Wait::new("the message").until(|| seen.stdout().contains("Restart")).await.unwrap();
        conn.item(
            sub,
            &item(
                12,
                Event::TurnCompleted {
                    turn_id: turn(),
                    usage: None,
                    changes: None,
                    context: None,
                    stop: None,
                    undo: None,
                },
            ),
        )
        .await;
        conn.until_closed().await;
    };
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    insta::assert_snapshot!(readable(&captured.stdout()));
}

/// A terminal on stdin and stdout whose keys the test types, and the plugin's file for
/// the text that the input row hands back, in `dir`.
fn row_context(env: &TestEnv, keys: &Arc<ScriptedKeys>, dir: &std::path::Path) -> Context {
    Context {
        term: terminal_facts(),
        keys: keys.clone(),
        screen: Arc::new(FixedScreen(Size { cols: 60, rows: 20 })),
        env: Env::fixed([(Var::DraftFile, dir.join("draft").into_os_string())]),
        ..env.context()
    }
}

fn completed_turn() -> Event {
    Event::TurnCompleted {
        turn_id: turn(),
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    }
}

#[tokio::test]
async fn keys_typed_while_the_prompt_goes_out_land_in_the_input_row() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let dir = tempfile::tempdir().unwrap();
    let ctx = row_context(&env, &keys, dir.path());
    let (mut out, captured) = capture();
    let seen = captured.clone();
    let typing = Arc::clone(&keys);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        // The reader started before the prompt went out.
        typing.type_bytes(b"and then").await;
        conn.reply(id, &sent(false)).await;
        let (sub, _) = conn.request().await;
        Wait::new("the row with the keys")
            .until(|| bare(&seen.stdout()).contains("\u{203a} and then"))
            .await
            .unwrap();
        conn.item(sub, &item(11, completed_turn())).await;
        conn.until_closed().await;
    };
    let line = command(&["send", "--", "hi"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(keys.kept(), 1, "one reader for the whole turn, which kept the typeahead");
    assert_eq!(keys.starts(), 1);
    assert_eq!(std::fs::read_to_string(dir.path().join("draft")).unwrap(), "and then");
}

#[tokio::test]
async fn a_prompt_that_the_daemon_refuses_hands_the_typed_keys_back() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let dir = tempfile::tempdir().unwrap();
    let ctx = row_context(&env, &keys, dir.path());
    let (mut out, _) = capture();
    let typing = Arc::clone(&keys);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        // Fewer keys than the queue holds: nothing reads them before the failure.
        typing.type_bytes(b"ahead").await;
        conn.fail(id, ErrorBody::new(ErrorCode::Busy, "the daemon is starting")).await;
        conn.until_closed().await;
    };
    let line = command(&["send", "--", "hi"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(std::fs::read_to_string(dir.path().join("draft")).unwrap(), "ahead");
}

#[tokio::test]
async fn with_render_turn_input_off_no_key_is_read() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let dir = tempfile::tempdir().unwrap();
    let mut settings = crate::settings::Settings::default();
    settings.render.turn_input = false;
    let ctx = Context { settings, ..row_context(&env, &keys, dir.path()) };
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &sent(false)).await;
        let (sub, _) = conn.request().await;
        conn.item(sub, &item(11, completed_turn())).await;
        conn.until_closed().await;
    };
    let line = command(&["send", "--", "hi"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(keys.starts(), 0, "the keys stay typeahead for the shell");
    assert!(!captured.stdout().contains('\u{203a}'), "{}", readable(&captured.stdout()));
}

// --- the prompts that a paste of `,` lines queues behind the first one

/// The second and the third turn: the queued prompts.
fn turn_2() -> efr_protocol::TurnId {
    "019a9b1c-3d00-7a10-8b20-000000000012".parse().unwrap()
}

fn turn_3() -> efr_protocol::TurnId {
    "019a9b1c-3d00-7a10-8b20-000000000013".parse().unwrap()
}

fn queued_send(turn_id: efr_protocol::TurnId) -> PromptSendResult {
    PromptSendResult { turn_id, queued: true, ..sent(false) }
}

fn started(turn_id: efr_protocol::TurnId) -> Event {
    Event::TurnStarted {
        turn_id,
        cwd: PathBuf::from("/etc/nginx"),
        scope: efr_protocol::Scope::Machine,
        settings: None,
    }
}

fn completed(turn_id: efr_protocol::TurnId, text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id, index: 0, text: text.to_owned() }
}

fn ended(turn_id: efr_protocol::TurnId) -> Event {
    Event::TurnCompleted {
        turn_id,
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    }
}

/// The next request, which must be `prompt.send`.
async fn prompt_send(conn: &mut Conn) -> (efr_protocol::RequestId, efr_protocol::PromptSend) {
    let (id, method) = conn.request().await;
    let Method::PromptSend(params) = method else {
        panic!("expected prompt.send, got {}", method.name());
    };
    (id, params)
}

#[tokio::test]
async fn without_the_input_row_each_queued_prompt_goes_when_the_turn_before_it_ended() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let vars = [
        (Var::Context, CONTEXT),
        (Var::LastCommand, "nginx -t"),
        (Var::Prompt, "first"),
        (Var::QueuedPrompts, "second\n\nthird; *.rs"),
    ];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, captured) = capture();
    let script = async {
        let mut sent_prompts = Vec::new();
        for reply in ["One.", "Two.", "Three."] {
            let mut conn = daemon.accept().await;
            sent_prompts.push(answer(&mut conn, sent(false), reply).await);
        }
        sent_prompts
    };
    let line = command(&["send"]);
    let (exit, sent_prompts) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    let texts: Vec<&str> = sent_prompts.iter().map(|params| params.text.as_str()).collect();
    assert_eq!(texts, ["first", "second", "third; *.rs"]);
    // The first goes where a prompt goes; the others follow it into its conversation,
    // with the same handover.
    assert_eq!(sent_prompts[0].conversation_id, None);
    for params in &sent_prompts[1..] {
        assert_eq!(params.conversation_id, Some(conversation()));
        assert!(!params.new_conversation);
        assert_eq!(params.last_command.as_deref(), Some("nginx -t"));
        assert_eq!(params.context.as_ref().unwrap().pwd, PathBuf::from("/etc/nginx"));
    }
    assert_eq!(captured.stdout(), "One.\nTwo.\nThree.\n");
}

#[tokio::test]
async fn with_the_input_row_the_queued_prompts_wait_behind_the_first_and_the_view_follows_them() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let dir = tempfile::tempdir().unwrap();
    let vars = [
        (Var::DraftFile, dir.path().join("draft").into_os_string()),
        (Var::QueuedPrompts, "second\nthird".into()),
    ];
    let ctx = Context { env: Env::fixed(vars), ..row_context(&env, &keys, dir.path()) };
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, first) = prompt_send(&mut conn).await;
        assert_eq!(first.text, "first");
        conn.reply(id, &sent(false)).await;
        // Both go at once, in their order, as Tab in the row queues a prompt.
        for (text, turn_id) in [("second", turn_2()), ("third", turn_3())] {
            let (id, params) = prompt_send(&mut conn).await;
            assert_eq!(params.text, text);
            assert_eq!(params.conversation_id, Some(conversation()));
            assert!(!params.new_conversation);
            conn.reply(id, &queued_send(turn_id)).await;
        }
        let (sub, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationSubscribe(_)), "{}", method.name());
        conn.item(sub, &item(11, completed(turn(), "One."))).await;
        conn.item(sub, &item(12, ended(turn()))).await;
        let mut seq = 13;
        for (turn_id, text) in [(turn_2(), "Two."), (turn_3(), "Three.")] {
            for event in [started(turn_id), completed(turn_id, text), ended(turn_id)] {
                conn.item(sub, &item(seq, event)).await;
                seq += 1;
            }
        }
        conn.until_closed().await;
    };
    let line = command(&["send", "--", "first"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    let shown = bare(&captured.stdout());
    let at = |text: &str| shown.find(text).unwrap_or_else(|| panic!("no {text}: {shown}"));
    assert!(at("One.") < at("Two.") && at("Two.") < at("Three."), "{shown}");
    assert!(shown.contains("queued: third"), "{shown}");
    assert!(!dir.path().join("draft").exists(), "nothing goes back to the shell");
}

#[tokio::test]
async fn a_queued_prompt_that_efrd_refuses_goes_into_the_row_with_the_ones_after_it() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let dir = tempfile::tempdir().unwrap();
    let vars = [
        (Var::DraftFile, dir.path().join("draft").into_os_string()),
        (Var::QueuedPrompts, "second\nthird".into()),
    ];
    let ctx = Context { env: Env::fixed(vars), ..row_context(&env, &keys, dir.path()) };
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = prompt_send(&mut conn).await;
        conn.reply(id, &sent(false)).await;
        let (id, params) = prompt_send(&mut conn).await;
        assert_eq!(params.text, "second");
        conn.fail(id, ErrorBody::new(ErrorCode::Overflow, "the queue is full")).await;
        // No third prompt: the subscription comes next.
        let (sub, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationSubscribe(_)), "{}", method.name());
        conn.item(sub, &item(11, completed_turn())).await;
        conn.until_closed().await;
    };
    let line = command(&["send", "--", "first"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    let shown = format!("{}{}", bare(&captured.stdout()), captured.stderr());
    assert!(shown.contains("not queued: the queue is full"), "{shown}");
    // The row's text goes back to the shell when efr ends, so nothing is lost.
    assert_eq!(std::fs::read_to_string(dir.path().join("draft")).unwrap(), "second\nthird");
}

#[tokio::test]
async fn the_queued_prompts_that_did_not_go_go_back_to_the_shell_as_comma_lines() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let dir = tempfile::tempdir().unwrap();
    let vars = [
        (Var::Prompt, "first".into()),
        (Var::QueuedPrompts, "second\nthird".into()),
        (Var::DraftFile, dir.path().join("draft").into_os_string()),
    ];
    let ctx = Context { env: Env::fixed(vars), ..env.context() };
    let (mut out, _) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = prompt_send(&mut conn).await;
        conn.fail(id, ErrorBody::new(ErrorCode::Busy, "the daemon is starting")).await;
        conn.until_closed().await;
    };
    let line = command(&["send"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    // The plugin puts `, ` before the text, so Enter sends both again as two prompts.
    assert_eq!(std::fs::read_to_string(dir.path().join("draft")).unwrap(), "second\n, third");
}
