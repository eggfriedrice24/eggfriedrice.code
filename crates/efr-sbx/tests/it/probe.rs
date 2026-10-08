//! The probe: ready here, and the fallback reasons it gives when a part is missing.
//! The failure cases stop before any launch, so they run without a ready sandbox.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use efr_protocol::{CacheMode, CheckOutcome};
use efr_sandbox::{ProbeFailure, ProbeReport};

use crate::support::{command, sandbox_or_skip, target_tmp};

/// The launcher of `EFR_TEST_SBX_BIN`, else the one cargo built with this test binary.
///
/// NOTE: `just test-sandbox` sets the variable to a private copy, because another cargo
/// command in the same target dir can link a new `target/debug/efr-sbx` while a test
/// runs it. Without the variable, a run of these tests must not share its target dir
/// with a build of efr-sbx.
fn launcher() -> PathBuf {
    crate::support::env_var("EFR_TEST_SBX_BIN")
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_efr-sbx")), PathBuf::from)
}

fn probe(args: &[&str]) -> ProbeReport {
    let dir = tempfile::Builder::new().prefix("p").tempdir_in(target_tmp()).unwrap();
    let output = command(launcher())
        .args(["probe", "--json", "--dir"])
        .arg(dir.path())
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    ProbeReport::from_json(&output.stdout).unwrap()
}

#[test]
fn probe_reports_ready_here() {
    let _ready = sandbox_or_skip!();
    let report = probe(&[]);
    assert_eq!(report.failure, None, "{report:#?}");
    assert!(report.checks.iter().all(|check| check.outcome == CheckOutcome::Ok), "{report:#?}");
    assert!(report.landlock_abi.unwrap_or_default() >= 9);
    assert!(report.launch_us.is_some());
    let names: Vec<&str> = report.checks.iter().map(|check| check.name.as_str()).collect();
    for name in ["platform", "landlock", "bwrap", "launcher", "zsh", "path", "self_test"] {
        assert!(names.contains(&name), "{name} missing: {names:?}");
    }
}

#[test]
fn probe_runs_the_self_test_and_the_launch_cost_in_the_configured_cache_mode() {
    let _ready = sandbox_or_skip!();
    let report = probe(&[]);
    assert_eq!(report.cache_mode, CacheMode::Tmp, "tmp is the default of sandbox.cache_mode");
    for (flag, mode) in [("overlay", CacheMode::Overlay), ("readonly", CacheMode::Readonly)] {
        let report = probe(&["--cache-mode", flag]);
        assert_eq!(report.failure, None, "{flag}: {report:#?}");
        assert_eq!(report.cache_mode, mode, "{report:#?}");
        assert!(report.warnings.is_empty(), "{report:#?}");
        assert!(report.launch_us.is_some(), "{report:#?}");
    }
}

/// The failed check `name`: its reason and its fix. A machine without Landlock fails
/// that check first, so the tests look at the check itself, not the first failure.
fn failed(report: &ProbeReport, name: &str) -> (String, String) {
    let check = report
        .checks
        .iter()
        .find(|check| check.name == name)
        .unwrap_or_else(|| panic!("no {name} check: {report:#?}"));
    assert_eq!(check.outcome, CheckOutcome::Fail, "{report:#?}");
    assert!(!report.status().available);
    (check.detail.clone().unwrap_or_default(), check.fix.clone().unwrap_or_default())
}

#[test]
fn probe_bwrap_missing_names_fix() {
    let report = probe(&["--bwrap", "/nonexistent/bwrap"]);
    let (reason, fix) = failed(&report, "bwrap");
    assert_eq!(reason, ProbeFailure::NoBwrap.reason());
    assert_eq!(reason, "bubblewrap is not installed");
    assert!(fix.contains("pacman -S bubblewrap"), "{fix}");
}

fn fake_bwrap(mode: u32) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new().prefix("b").tempdir_in(target_tmp()).unwrap();
    let path = dir.path().join("bwrap");
    fs::write(&path, "#!/bin/sh\necho 'bubblewrap 0.13.0'\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    (dir, path)
}

#[test]
fn probe_setuid_bwrap_unavailable() {
    let (_dir, bwrap) = fake_bwrap(0o4755);
    let report = probe(&["--bwrap", bwrap.to_str().unwrap()]);
    let (reason, _) = failed(&report, "bwrap");
    assert_eq!(reason, ProbeFailure::SetuidBwrap.reason());
}

#[test]
fn probe_untrusted_bwrap_unavailable() {
    let (_dir, bwrap) = fake_bwrap(0o755);
    let report = probe(&["--bwrap", bwrap.to_str().unwrap()]);
    let (reason, _) = failed(&report, "bwrap");
    assert_eq!(reason, ProbeFailure::BwrapNotTrusted { path: bwrap }.reason());
}

#[test]
fn probe_relative_path_entry_unavailable() {
    let report = probe(&["--shell-path", "bin:/usr/bin:/bin"]);
    let (reason, _) = failed(&report, "path");
    assert_eq!(reason, ProbeFailure::RelativePath { entry: "bin".to_owned() }.reason());
}

#[test]
fn escape_launcher_in_write_root_refuses_auto() {
    // The `just run` case: the launcher sits in target/ of a registered project.
    let exe = launcher().canonicalize().unwrap();
    let project = exe.ancestors().nth(2).unwrap_or(Path::new("/")).to_path_buf();
    let report = probe(&["--write-root", project.to_str().unwrap()]);
    let (reason, _) = failed(&report, "launcher");
    let expected = ProbeFailure::InWriteRoot { what: "efr-sbx".to_owned(), path: exe };
    assert_eq!(reason, expected.reason());
    assert!(reason.contains("writable project"), "{reason}");
}
