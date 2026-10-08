use std::path::PathBuf;

use efr_protocol::{
    AdminStatusResult, CacheMode, ConfigFileError, ConfigStatus, ErrorBody, ErrorCode, Method,
    NetworkMode, Origin, ProviderStatus, SandboxStatus,
};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use crate::context::Context;
use crate::error::Exit;
use crate::run;
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
        }],
        catalog: Some(efr_protocol::CatalogStatus {
            origin: efr_protocol::CatalogOrigin::Cache,
            fetched_at: Some(now() - SignedDuration::from_hours(26)),
        }),
        roots: None,
        config: None,
        sandbox: None,
        sandbox_paths: None,
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
