//! Running the sandbox probe (efr's auto spec, section 12.1): efrd's own checks first
//! (`sandbox.enabled`, the launcher found, outside every write root, its copy equal to
//! its source), then `efr-sbx probe --json` from the copy, which checks the platform,
//! Landlock, bwrap, zsh and `PATH` and runs one real launch with a self-test inside.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{CacheMode, CheckOutcome, SandboxCheck};
use efr_sandbox::{ProbeFailure, ProbeReport, is_within};
use efr_stdx::time::Clock;

/// How long the launcher's probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// The most bytes of the probe's report.
const MAX_REPORT: usize = 1024 * 1024;

/// What one probe needs.
#[derive(Debug, Clone)]
pub(crate) struct ProbeInput {
    /// `sandbox.enabled`.
    pub(crate) enabled: bool,
    /// The installed launcher, when one was found.
    pub(crate) source: Option<PathBuf>,
    /// Its copy in efr's runtime root.
    pub(crate) copy: PathBuf,
    /// A private directory for the probe's fake call, `$S/sandbox/probe`.
    pub(crate) dir: PathBuf,
    /// `sandbox.bwrap`.
    pub(crate) bwrap: Option<PathBuf>,
    /// The hidden shell's zsh.
    pub(crate) zsh: Option<PathBuf>,
    pub(crate) home: PathBuf,
    pub(crate) user_runtime: PathBuf,
    /// `sandbox.cache_mode`.
    pub(crate) cache_mode: CacheMode,
    /// Every write root of the user's projects and `sandbox.write_roots`.
    pub(crate) write_roots: Vec<PathBuf>,
    /// The hidden shell's `PATH`.
    pub(crate) shell_path: String,
}

/// The report of efrd's own checks, or `None` when they pass and the launcher's probe
/// must run.
pub(crate) fn own_checks(input: &ProbeInput, copy_matches: bool) -> Option<ProbeReport> {
    let failure = if !input.enabled {
        ProbeFailure::Disabled
    } else if let Some(source) = &input.source {
        if let Some(root) = input.write_roots.iter().find(|root| is_within(source, root)) {
            tracing::debug!(root = %root.display(), "the launcher lies in a write root");
            ProbeFailure::InWriteRoot { what: "efr-sbx".to_owned(), path: source.clone() }
        } else if !copy_matches {
            ProbeFailure::LauncherMismatch
        } else {
            return None;
        }
    } else {
        ProbeFailure::NoLauncher
    };
    Some(failed(failure, input.cache_mode))
}

/// A report that holds only `failure`.
pub(crate) fn failed(failure: ProbeFailure, cache_mode: CacheMode) -> ProbeReport {
    ProbeReport {
        checks: vec![failure.check()],
        failure: Some(failure),
        cache_mode,
        ..ProbeReport::default()
    }
}

/// The arguments of `efr-sbx probe` for `input`.
pub(crate) fn args(input: &ProbeInput) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> =
        vec!["probe".into(), "--json".into(), "--dir".into(), input.dir.clone().into()];
    let mut flag = |name: &str, value: std::ffi::OsString| {
        args.push(name.into());
        args.push(value);
    };
    if let Some(bwrap) = &input.bwrap {
        flag("--bwrap", bwrap.clone().into());
    }
    if let Some(zsh) = &input.zsh {
        flag("--zsh", zsh.clone().into());
    }
    flag("--home", input.home.clone().into());
    flag("--user-runtime", input.user_runtime.clone().into());
    flag("--cache-mode", input.cache_mode.as_str().into());
    for root in &input.write_roots {
        flag("--write-root", root.clone().into());
    }
    flag("--shell-path", input.shell_path.clone().into());
    args
}

/// Runs the launcher's probe from its copy and reads its report. A probe that does
/// not run, ends in time or print a report is a failure of its own.
pub(crate) async fn run_launcher(input: &ProbeInput, clock: &Arc<dyn Clock>) -> ProbeReport {
    if let Err(error) = make_private_dir(&input.dir).await {
        return failed(
            ProbeFailure::ProbeFailed {
                detail: format!("{} cannot be made: {error}", input.dir.display()),
            },
            input.cache_mode,
        );
    }
    let cwd = input.dir.clone();
    let mut command = efr_stdx::process::command(&input.copy, &cwd);
    command
        .args(args(input))
        .env("PATH", &input.shell_path)
        .env("HOME", &input.home)
        .env("XDG_RUNTIME_DIR", &input.user_runtime)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match clock.timeout(PROBE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            let detail = format!("efr-sbx did not start: {error}");
            return failed(ProbeFailure::ProbeFailed { detail }, input.cache_mode);
        }
        Err(_) => {
            let detail = format!("efr-sbx did not answer within {}s", PROBE_TIMEOUT.as_secs());
            return failed(ProbeFailure::ProbeFailed { detail }, input.cache_mode);
        }
    };
    let report = (output.stdout.len() <= MAX_REPORT)
        .then(|| ProbeReport::from_json(&output.stdout).ok())
        .flatten();
    match report {
        Some(report) => report,
        None => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let first = stderr.lines().next().unwrap_or("no output").chars().take(200).collect();
            tracing::warn!(status = ?output.status, stderr = %stderr, "the sandbox probe printed no report");
            failed(ProbeFailure::ProbeFailed { detail: first }, input.cache_mode)
        }
    }
}

/// The checks of a report that the launcher's own checks did not list, with efrd's
/// checks in front.
pub(crate) fn with_own_checks(mut report: ProbeReport, input: &ProbeInput) -> ProbeReport {
    let mut own = vec![SandboxCheck {
        name: "enabled".to_owned(),
        outcome: CheckOutcome::Ok,
        detail: Some("sandbox.enabled = true".to_owned()),
        fix: None,
    }];
    if let Some(source) = &input.source {
        own.push(SandboxCheck {
            name: "launcher_copy".to_owned(),
            outcome: CheckOutcome::Ok,
            detail: Some(format!(
                "{} (from {}, sha256 ok)",
                input.copy.display(),
                source.display()
            )),
            fix: None,
        });
    }
    own.append(&mut report.checks);
    report.checks = own;
    report
}

async fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests;
