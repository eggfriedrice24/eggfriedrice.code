use std::path::PathBuf;

use efr_protocol::{
    AdminSandboxCheckResult, CacheMode, CheckOutcome, ErrorBody, ErrorCode, ExitKind, Method, Mode,
    NetworkMode, Origin, SandboxCheck, SandboxExplainResult, SandboxPathRole, SandboxStatus,
};
use pretty_assertions::assert_eq;

use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command};

fn ready() -> SandboxStatus {
    SandboxStatus {
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
    }
}

fn ok(name: &str, detail: &str) -> SandboxCheck {
    SandboxCheck {
        name: name.to_owned(),
        outcome: CheckOutcome::Ok,
        detail: Some(detail.to_owned()),
        fix: None,
    }
}

#[tokio::test]
async fn sandbox_check_lists_checks() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let result = AdminSandboxCheckResult {
        status: ready(),
        checks: vec![
            ok("landlock", "Landlock ABI 10, errata 0xf"),
            ok("bwrap", "bubblewrap /usr/bin/bwrap 0.13.0, not setuid"),
        ],
        launch_us: Some(3_600),
        snapshot_launch_us: Some(14_800),
    };
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Cli);
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminSandboxCheck(_)), "{}", method.name());
        conn.reply(id, &result).await;
        conn.until_closed().await;
    };
    let line = command(&["sandbox", "check"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        captured.stdout(),
        "\
ok    Landlock ABI 10, errata 0xf
ok    bubblewrap /usr/bin/bwrap 0.13.0, not setuid
launch 3.6 ms, with your rc snapshot 14.8 ms
auto: ready
"
    );
    assert_eq!(captured.stderr(), "");
}

#[tokio::test]
async fn an_unavailable_sandbox_prints_the_fix_and_exits_with_one() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let result = AdminSandboxCheckResult {
        status: SandboxStatus {
            fix: Some("Arch: pacman -S bubblewrap".to_owned()),
            ..SandboxStatus::unavailable("bubblewrap is not installed")
        },
        checks: vec![SandboxCheck {
            name: "bwrap".to_owned(),
            outcome: CheckOutcome::Fail,
            detail: Some("bubblewrap is not installed".to_owned()),
            fix: Some("Arch: pacman -S bubblewrap".to_owned()),
        }],
        launch_us: None,
        snapshot_launch_us: None,
    };
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &result).await;
        conn.until_closed().await;
    };
    let line = command(&["sandbox", "check"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Invalid);
    assert_eq!(
        captured.stdout(),
        "\
fail  bubblewrap is not installed
      fix: Arch: pacman -S bubblewrap
auto: unavailable: bubblewrap is not installed; turns run as cautious
"
    );
    // The listing already says why; no second message.
    assert_eq!(captured.stderr(), "");
}

#[tokio::test]
async fn a_daemon_without_the_method_names_its_refusal() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        let refusal =
            ErrorBody::new(ErrorCode::Invalid, "this daemon does not answer this method yet");
        conn.fail(id, refusal).await;
        conn.until_closed().await;
    };
    let line = command(&["sandbox", "check"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert!(captured.stderr().contains("this daemon does not answer this method yet"));
}

#[tokio::test]
async fn explain_asks_from_the_current_directory_and_prints_the_answer() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::SandboxExplain(params) = method else {
            panic!("expected sandbox.explain, got {}", method.name());
        };
        // A relative path is the current directory's, which also picks the project.
        assert_eq!(params.path, PathBuf::from("/home/user/project/../.zshrc"));
        assert_eq!(params.cwd, Some(PathBuf::from("/home/user/project")));
        let answer = SandboxExplainResult {
            path: PathBuf::from("/home/user/.zshrc"),
            project: Some(PathBuf::from("/home/user/project")),
            mode: Mode::Auto,
            role: SandboxPathRole::Floor,
            read: true,
            write: false,
            reason: "a shell startup file (floor)".to_owned(),
            write_exit: Some(ExitKind::Persistence),
        };
        conn.reply(id, &answer).await;
        conn.until_closed().await;
    };
    let line = command(&["sandbox", "explain", "../.zshrc"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        captured.stdout(),
        "\
~/.zshrc, from ~/project in auto:
  read   yes
  write  no: a shell startup file (floor); a write is a persistence exit, user only
"
    );
}

#[tokio::test]
async fn sandbox_commands_without_a_daemon_exit_with_three() {
    let env = TestEnv::new();
    let ctx = env.context();
    for args in [&["sandbox", "check"][..], &["sandbox", "explain", "/etc/hosts"][..]] {
        let (mut out, _) = capture();
        let exit = run::run(&command(args), &ctx, &mut out).await;
        assert_eq!(exit, Exit::NotRunning, "{args:?}");
    }
}
