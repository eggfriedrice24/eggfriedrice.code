use std::path::PathBuf;

use efr_protocol::{
    AdminStatusResult, BusProxyOff, CacheMode, ConfigFileError, ConfigStatus, ErrorBody, ErrorCode,
    FullModeStatus, FullRefusal, HeldConfig, LoginKind, Method, NetworkMode, Origin,
    ProviderStatus, SandboxStatus,
};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::settings::Settings;
use crate::testing::{InstantClock, TestEnv, capture, command, now};
use std::sync::Arc;

fn result() -> AdminStatusResult {
    AdminStatusResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        version: "0.1.0".to_owned(),
        protocol: efr_protocol::PROTOCOL_VERSION,
        pid: 777,
        started_at: now() - SignedDuration::from_secs(90),
        screen_backend: "ghostty".to_owned(),
        conversations: 0,
        shells: 0,
        providers: vec![ProviderStatus {
            provider: "openai".to_owned(),
            logged_in: false,
            expires_at: None,
            active: false,
            login: None,
            key_hint: None,
            key_refused_at: None,
        }],
        catalog: Some(efr_protocol::CatalogStatus {
            provider: None,
            origin: efr_protocol::CatalogOrigin::Cache,
            fetched_at: Some(now() - SignedDuration::from_hours(26)),
        }),
        roots: None,
        config: None,
        sandbox: None,
        sandbox_paths: None,
        full_mode: None,
    }
}

#[tokio::test]
async fn status_shows_the_daemons_answer_and_its_socket() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Cli);
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminStatus(_)), "{}", method.name());
        conn.reply(id, &result()).await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let text = captured.stdout().replace(&env.socket().display().to_string(), "<socket>");
    insta::assert_snapshot!(text);
    // No provider has credentials, so every prompt would fail until a login.
    assert_eq!(
        captured.stderr(),
        "efr: no model provider is logged in; log in with: efr login openai\n"
    );
}

#[tokio::test]
async fn a_logged_in_provider_needs_no_login_hint() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let mut status = result();
    status.providers.push(ProviderStatus {
        provider: "openai-subscription".to_owned(),
        logged_in: true,
        expires_at: None,
        active: false,
        login: None,
        key_hint: None,
        key_refused_at: None,
    });
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &status).await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(captured.stderr(), "");
}

/// The three providers of a daemon that uses `anthropic-api`, with a subscription login
/// and no key for it.
fn three_providers(anthropic_key: Option<&str>) -> Vec<ProviderStatus> {
    let provider = |id: &str| ProviderStatus {
        provider: id.to_owned(),
        logged_in: false,
        expires_at: None,
        active: false,
        login: None,
        key_hint: None,
        key_refused_at: None,
    };
    vec![
        ProviderStatus {
            logged_in: true,
            expires_at: Some(now() + SignedDuration::from_hours(150)),
            login: Some(LoginKind::Subscription),
            ..provider("openai-subscription")
        },
        provider("openai-api"),
        ProviderStatus {
            logged_in: anthropic_key.is_some(),
            active: true,
            login: anthropic_key.map(|_| LoginKind::ApiKey),
            key_hint: anthropic_key.map(str::to_owned),
            ..provider("anthropic-api")
        },
    ]
}

#[tokio::test]
async fn status_shows_every_provider_with_its_key_hint_and_the_active_one() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let mut status = result();
    status.providers = three_providers(Some("sk-ant-...a1b2"));
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &status).await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let stdout = captured.stdout();
    let rows: Vec<&str> = stdout.lines().filter(|line| line.starts_with("provider")).collect();
    assert_eq!(
        rows,
        [
            "provider       openai-subscription: logged in, token expires 2026-10-10T18:00:00Z (in 6d 6h)",
            "provider       openai-api: not logged in",
            "provider       anthropic-api: logged in, key sk-ant-...a1b2 (active)",
        ]
    );
    assert_eq!(captured.stderr(), "");
}

#[tokio::test]
async fn an_active_provider_without_a_login_gets_the_hint_of_its_own_login() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let mut status = result();
    status.providers = three_providers(None);
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &status).await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        captured.stderr(),
        "efr: anthropic-api is not logged in; log in with: efr login anthropic\n"
    );
}

#[tokio::test]
async fn status_shows_the_config_file_its_reload_error_and_the_keys_that_wait_for_a_restart() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let mut status = result();
    status.config = Some(ConfigStatus {
        path: PathBuf::from("/c/efr/config.toml"),
        exists: true,
        symlink_target: Some(PathBuf::from("/home/u/dotfiles/efr/config.toml")),
        reload_error: Some(ConfigFileError {
            message: "unknown field `idle_minuets`".to_owned(),
            line: Some(3),
            column: Some(1),
            key: Some("shell.idle_minuets".to_owned()),
        }),
        restart_needed: vec!["screen".to_owned(), "model.provider".to_owned()],
        held: None,
    });
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &status).await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let stdout = captured.stdout();
    assert!(
        stdout.ends_with(
            "\
config         /c/efr/config.toml -> /home/u/dotfiles/efr/config.toml
config error   unknown field `idle_minuets` (shell.idle_minuets, line 3, column 1)
restart needed screen, model.provider
"
        ),
        "{stdout}"
    );
}

#[tokio::test]
async fn status_shows_sandbox_line() {
    let ready = SandboxStatus {
        available: true,
        reason: None,
        fix: None,
        landlock_abi: Some(10),
        errata: Some(0xf),
        bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        bwrap_version: Some("0.13.0".to_owned()),
        cache_mode: CacheMode::Overlay,
        network_mode: NetworkMode::None,
        warnings: Vec::new(),
        netview: false,
        netview_off: false,
        proxy: false,
        bus_proxy: false,
        bus_proxy_off: None,
    };
    let down = SandboxStatus {
        fix: Some("a newer kernel".to_owned()),
        ..SandboxStatus::unavailable("Landlock ABI 6 found; auto needs 9 (Linux 7.1)")
    };
    let mut shown = Vec::new();
    for sandbox in [Some(ready), Some(down), None] {
        let env = TestEnv::new();
        let daemon = env.listen();
        let ctx = env.context();
        let (mut out, captured) = capture();
        let status = AdminStatusResult { sandbox, ..result() };
        let script = async {
            let mut conn = daemon.accept().await;
            let (id, _) = conn.request().await;
            conn.reply(id, &status).await;
            conn.until_closed().await;
        };
        let line = command(&["status"]);
        let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
        assert_eq!(exit, Exit::Success);
        let stdout = captured.stdout();
        shown.push(
            stdout
                .lines()
                .filter(|line| line.starts_with("sandbox"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    assert_eq!(
        shown,
        [
            "sandbox        ready (Landlock ABI 10, bubblewrap 0.13.0, caches overlay, network none)",
            "sandbox        unavailable: Landlock ABI 6 found; auto needs 9 (Linux 7.1); auto runs as cautious\nsandbox fix    a newer kernel",
            // A daemon from before the sandbox reports none.
            "",
        ]
    );
}

#[tokio::test]
async fn status_shows_netview() {
    let ready = SandboxStatus {
        available: true,
        reason: None,
        fix: None,
        landlock_abi: Some(10),
        errata: Some(0xf),
        bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        bwrap_version: Some("0.13.0".to_owned()),
        cache_mode: CacheMode::Overlay,
        network_mode: NetworkMode::None,
        warnings: Vec::new(),
        netview: true,
        netview_off: false,
        proxy: false,
        bus_proxy: false,
        bus_proxy_off: None,
    };
    let failed = SandboxStatus {
        warnings: vec![
            "the network view's self-test failed, so ss and ip need a review: tcp4: errno 1"
                .to_owned(),
        ],
        netview: false,
        ..ready.clone()
    };
    // A daemon from before the network view sends no `netview`, which reads as false.
    let older = SandboxStatus { netview: false, ..ready.clone() };
    let down = SandboxStatus::unavailable("bubblewrap is not installed");
    // The user's `network.netview = false` wins over a passed self-test.
    let off = SandboxStatus { netview_off: true, ..ready.clone() };
    let mut shown = Vec::new();
    for sandbox in [ready, failed, older, down, off] {
        let env = TestEnv::new();
        let daemon = env.listen();
        let ctx = env.context();
        let (mut out, captured) = capture();
        let status = AdminStatusResult { sandbox: Some(sandbox), ..result() };
        let script = async {
            let mut conn = daemon.accept().await;
            let (id, _) = conn.request().await;
            conn.reply(id, &status).await;
            conn.until_closed().await;
        };
        let line = command(&["status"]);
        let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
        assert_eq!(exit, Exit::Success);
        let stdout = captured.stdout();
        let rows: Vec<&str> = stdout
            .lines()
            .filter(|line| line.starts_with("sandbox") || line.starts_with("netview"))
            .collect();
        shown.push(rows.join("\n"));
    }
    insta::assert_snapshot!(shown.join("\n---\n"));
}

#[tokio::test]
async fn status_shows_bus_filter() {
    let ready = SandboxStatus {
        available: true,
        reason: None,
        fix: None,
        landlock_abi: Some(10),
        errata: Some(0xf),
        bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        bwrap_version: Some("0.13.0".to_owned()),
        cache_mode: CacheMode::Overlay,
        network_mode: NetworkMode::None,
        warnings: Vec::new(),
        netview: true,
        netview_off: false,
        proxy: false,
        bus_proxy: true,
        bus_proxy_off: None,
    };
    let off =
        |reason| SandboxStatus { bus_proxy: false, bus_proxy_off: Some(reason), ..ready.clone() };
    // A daemon from before the bus filter sends neither field.
    let older = SandboxStatus { bus_proxy: false, ..ready.clone() };
    // An unavailable sandbox has no bus filter row.
    let down = SandboxStatus::unavailable("bubblewrap is not installed");
    let mut shown = Vec::new();
    for sandbox in [
        ready.clone(),
        off(BusProxyOff::Config),
        off(BusProxyOff::NoProgram),
        off(BusProxyOff::SelfTest),
        off(BusProxyOff::OldVersion),
        off(BusProxyOff::Unknown),
        older,
        down,
    ] {
        let env = TestEnv::new();
        let daemon = env.listen();
        let ctx = env.context();
        let (mut out, captured) = capture();
        let status = AdminStatusResult { sandbox: Some(sandbox), ..result() };
        let script = async {
            let mut conn = daemon.accept().await;
            let (id, _) = conn.request().await;
            conn.reply(id, &status).await;
            conn.until_closed().await;
        };
        let line = command(&["status"]);
        let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
        assert_eq!(exit, Exit::Success);
        let stdout = captured.stdout();
        let rows: Vec<&str> = stdout
            .lines()
            .filter(|line| line.starts_with("sandbox ") || line.starts_with("bus filter"))
            .collect();
        shown.push(rows.join("\n"));
    }
    insta::assert_snapshot!(shown.join("\n---\n"));
}

#[tokio::test]
async fn status_without_a_daemon_exits_with_three() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["status"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::NotRunning);
    assert!(
        captured.stderr().ends_with("efr: start the daemon with: systemctl --user start efrd, or `just run` in the efr checkout for a foreground one\n")
    );
    assert_eq!(captured.stdout(), "");
}

#[tokio::test]
async fn a_status_refused_for_lack_of_scope_exits_with_one() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.fail(id, ErrorBody::new(ErrorCode::Forbidden, "admin methods need the Unix socket"))
            .await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert!(captured.stderr().contains("forbidden: admin methods need the Unix socket"));
}

#[tokio::test]
async fn a_daemon_that_does_not_answer_times_out() {
    let env = TestEnv::new();
    // Accepts connections but never reads them.
    let _listener = tokio::net::UnixListener::bind(env.socket()).unwrap();
    let ctx = Context { clock: Arc::new(InstantClock), ..env.context() };
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["status"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::DaemonError);
    // The connect or the hello times out, whichever the clock reaches first.
    let stderr = captured.stderr();
    assert!(stderr.contains("did not finish within 5s\n"), "{stderr}");
    assert!(
        stderr.ends_with("efr: the daemon is not answering; its log: journalctl --user -u efrd\n")
    );
}

#[tokio::test]
async fn a_refused_key_shows_when_it_was_refused_and_the_active_one_gets_a_warning() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let mut status = result();
    status.providers = three_providers(Some("sk-ant-...a1b2"));
    status.providers[2].key_refused_at = Some(now() - SignedDuration::from_mins(5));
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &status).await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let stdout = captured.stdout();
    let rows: Vec<&str> = stdout.lines().filter(|line| line.starts_with("provider")).collect();
    insta::assert_snapshot!(format!("{}\n---\n{}", rows.join("\n"), captured.stderr()));
}

/// The rows of `efr status` after the `config` row, for a daemon that reports
/// `full_mode` and `held`, with a client whose `config.toml` holds `config`.
async fn full_rows(
    full_mode: Option<FullModeStatus>,
    held: Option<HeldConfig>,
    config: Option<&str>,
) -> String {
    let env = TestEnv::new();
    let daemon = env.listen();
    let settings = match config {
        Some(text) => Settings::parse(&env.dirs.config().join("config.toml"), text),
        None => Settings::default(),
    };
    assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    let ctx = Context { settings, ..env.context() };
    let (mut out, captured) = capture();
    let mut status = result();
    status.full_mode = full_mode;
    status.config = Some(ConfigStatus {
        path: PathBuf::from("/c/efr/config.toml"),
        exists: true,
        symlink_target: None,
        reload_error: None,
        restart_needed: Vec::new(),
        held,
    });
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &status).await;
        conn.until_closed().await;
    };
    let line = command(&["status"]);
    let (exit, ()) = tokio::join!(Box::pin(run::run(&line, &ctx, &mut out)), script);
    assert_eq!(exit, Exit::Success);
    let stdout = captured.stdout();
    let at = stdout.find("\nconfig ").unwrap();
    stdout[at + 1..].lines().skip(1).map(|line| format!("{line}\n")).collect()
}

#[tokio::test]
async fn efr_status_shows_the_full_mode_gate_on_off_and_refused_and_a_held_reload() {
    let on = FullModeStatus { allowed: true, refused: None };
    assert_eq!(full_rows(Some(on), None, None).await, "full mode      allowed\n");
    let off = FullModeStatus { allowed: false, refused: None };
    assert_eq!(
        full_rows(Some(off), None, None).await,
        "full mode      off (permissions.allow_full is false)\n"
    );
    let root = FullModeStatus { allowed: true, refused: Some(FullRefusal::Root) };
    assert_eq!(
        full_rows(Some(root), None, None).await,
        "full mode      refused (efrd runs as root)\n"
    );
    // A daemon from before full mode says nothing about it.
    assert_eq!(full_rows(None, None, None).await, "");
    let held = HeldConfig {
        loosens: vec!["permissions.rules".to_owned(), "permissions.allow_full".to_owned()],
    };
    assert_eq!(
        full_rows(Some(on), Some(held), None).await,
        "config held    config.toml changed while a full turn ran, and the change loosens \
         permissions.rules, permissions.allow_full. If you did not make this change, take it \
         out of the file. Then run efr config reload.\n\
         full mode      allowed\n"
    );
}

#[tokio::test]
async fn efr_status_shows_a_default_mode_of_full_from_the_config_file() {
    const FULL: &str = "[permissions]\nmode = \"full\"\nallow_full = true\n";
    let on = FullModeStatus { allowed: true, refused: None };
    assert_eq!(
        full_rows(Some(on), None, Some(FULL)).await,
        "full mode      allowed\ndefault mode   full\n"
    );
    let undo_off = FullModeStatus { allowed: true, refused: Some(FullRefusal::UndoOff) };
    assert_eq!(
        full_rows(Some(undo_off), None, Some(FULL)).await,
        "full mode      refused (undo.enabled is false, so efr undo cannot revert the turn)\n\
         default mode   full; efrd cannot run it, so a turn that names no mode runs as auto\n"
    );
    // Another default mode, with the gate on, adds no row.
    let auto = "[permissions]\nmode = \"auto\"\nallow_full = true\n";
    assert_eq!(full_rows(Some(on), None, Some(auto)).await, "full mode      allowed\n");
}
