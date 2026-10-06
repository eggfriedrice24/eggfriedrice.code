//! The daemon's side of the probe: its own checks, and the report of the launcher's
//! probe, here from a fake launcher that prints a report as the real one does.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::CacheMode;
use efr_sandbox::{ProbeFailure, ProbeReport};
use efr_test_support::TestClock;

use super::{ProbeInput, args, own_checks, run_launcher, with_own_checks};

fn input(dir: &Path, source: Option<PathBuf>) -> ProbeInput {
    ProbeInput {
        enabled: true,
        source,
        copy: dir.join("efr-sbx"),
        dir: dir.join("probe"),
        bwrap: None,
        zsh: Some(PathBuf::from("/usr/bin/zsh")),
        home: dir.to_path_buf(),
        user_runtime: dir.join("xrt"),
        cache_mode: CacheMode::Tmp,
        write_roots: vec![dir.join("p")],
        shell_path: "/usr/bin:/bin".to_owned(),
    }
}

/// A launcher that prints `stdout` for its probe.
fn fake_launcher(dir: &Path, stdout: &str) -> PathBuf {
    let path = dir.join("efr-sbx");
    std::fs::write(&path, format!("#!/bin/sh\ncat <<'REPORT'\n{stdout}\nREPORT\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn report_of(failure: ProbeFailure) -> String {
    let report = ProbeReport {
        checks: vec![failure.check()],
        failure: Some(failure),
        landlock_abi: Some(8),
        ..ProbeReport::default()
    };
    String::from_utf8(report.to_json().unwrap()).unwrap()
}

#[test]
fn efrds_own_checks_come_first() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let disabled = ProbeInput { enabled: false, ..input(dir, Some(dir.join("src"))) };
    assert_eq!(own_checks(&disabled, true).unwrap().failure, Some(ProbeFailure::Disabled));
    assert_eq!(
        own_checks(&input(dir, None), true).unwrap().failure,
        Some(ProbeFailure::NoLauncher)
    );
    let in_root = input(dir, Some(dir.join("p/target/debug/efr-sbx")));
    assert!(matches!(
        own_checks(&in_root, true).unwrap().failure,
        Some(ProbeFailure::InWriteRoot { what, .. }) if what == "efr-sbx"
    ));
    let changed = input(dir, Some(dir.join("lib/efr-sbx")));
    assert_eq!(own_checks(&changed, false).unwrap().failure, Some(ProbeFailure::LauncherMismatch));
    assert!(own_checks(&changed, true).is_none(), "then the launcher's probe runs");
}

#[test]
fn the_probe_gets_every_fact_of_the_hidden_shell() {
    let dir = Path::new("/s");
    let args: Vec<String> =
        args(&input(dir, None)).iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert_eq!(
        args,
        [
            "probe",
            "--json",
            "--dir",
            "/s/probe",
            "--zsh",
            "/usr/bin/zsh",
            "--home",
            "/s",
            "--user-runtime",
            "/s/xrt",
            "--cache-mode",
            "tmp",
            "--write-root",
            "/s/p",
            "--shell-path",
            "/usr/bin:/bin",
        ]
    );
}

#[tokio::test]
async fn probe_abi8_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    fake_launcher(dir.path(), &report_of(ProbeFailure::AbiLow { found: 8 }));
    let clock = TestClock::new().shared();
    let report = run_launcher(&input(dir.path(), None), &clock).await;
    let status = report.status();
    assert!(!status.available);
    assert_eq!(status.reason.as_deref(), Some("Landlock ABI 8 found; auto needs 9 (Linux 7.1)"));
    assert_eq!(status.fix.as_deref(), Some("use a newer kernel"));
    assert_eq!(status.landlock_abi, Some(8));
}

#[tokio::test]
async fn probe_userns_denied_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    for (failure, reason) in [
        (ProbeFailure::UsernsOff, "unprivileged user namespaces are off"),
        (ProbeFailure::AppArmor, "AppArmor blocks user namespaces for bwrap"),
    ] {
        fake_launcher(dir.path(), &report_of(failure));
        let clock = TestClock::new().shared();
        let status = run_launcher(&input(dir.path(), None), &clock).await.status();
        assert!(!status.available);
        assert!(status.reason.as_deref().unwrap().starts_with(reason), "{status:?}");
        assert!(status.fix.is_some());
    }
}

#[tokio::test]
async fn a_probe_without_a_report_is_a_failure_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    fake_launcher(dir.path(), "not json");
    let clock = TestClock::new().shared();
    let report = run_launcher(&input(dir.path(), None), &clock).await;
    assert!(matches!(report.failure, Some(ProbeFailure::ProbeFailed { .. })), "{report:?}");
    let missing = tempfile::tempdir().unwrap();
    let report = run_launcher(&input(missing.path(), None), &clock).await;
    assert!(matches!(report.failure, Some(ProbeFailure::ProbeFailed { .. })), "{report:?}");
    // A ready report gets efrd's own checks in front.
    let ready =
        with_own_checks(ProbeReport::default(), &input(dir.path(), Some(dir.path().join("src"))));
    let names: Vec<&str> = ready.checks.iter().map(|check| check.name.as_str()).collect();
    assert_eq!(names, ["enabled", "launcher_copy"]);
}
