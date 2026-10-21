//! The surface guard of a contained job while it runs (efr's auto spec, section 3.18,
//! phase C).
//!
//! A job runs for hours with no turn around it, and its launcher's own guard compares
//! the git surface only at the job's end. So efrd runs `efr-sbx guard-check` for each
//! running contained job: every [`GUARD_INTERVAL`] (which covers the user's own
//! terminal and editor), and before each contained call of any conversation whose guard
//! roots overlap the job's, so that call's guard never takes an entry that the job
//! planted as its baseline. At a restart it runs once for each job that left no
//! `result.json` (`JobTable::recover`). An exit child job has no guard: it runs as the
//! user.
//!
//! The check runs as the user, outside any sandbox, with efrd's small environment, as
//! the launcher does. It compares the job's record of its start (in its state dir) with
//! the surface now, moves each entry that breaks a rule to the job's quarantine, and
//! writes `guard.json`. A break stops the job (`JobStopBy::Guard`), and its changes go
//! into the job's `job_ended`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{JobStopBy, SurfaceChange};
use efr_stdx::time::Clock;
use serde::Deserialize;

use super::{JobKey, JobTable, RunningJob, SpawnEnv};

/// How often efrd checks the git surface of each running contained job.
pub(crate) const GUARD_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The subcommand of the launcher.
const GUARD_CHECK: &str = "guard-check";

/// The exit status of `efr-sbx guard-check` when it found a change.
const BROKEN: i32 = 3;

/// The longest that one guard check may take. It waits at most 5 s for the job's
/// guard lock, then scans the job's guard roots.
const CHECK_LIMIT: Duration = Duration::from_secs(60);

/// The most bytes of a `guard.json` that efrd reads.
const MAX_REPORT_BYTES: usize = 1024 * 1024;

/// `guard.json`: what one check found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct GuardReport {
    /// Each change that breaks a rule of the guard.
    pub(crate) changes: Vec<SurfaceChange>,
    /// True when an entry moved to the quarantine.
    #[serde(default)]
    pub(crate) quarantined: bool,
}

/// What one check of a job found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Checked {
    /// No change breaks a rule.
    Clean,
    /// Changes broke a rule; the check moved what it could to the quarantine.
    Broken(GuardReport),
    /// The check could not run or failed; the job's own end guard still runs.
    Failed,
}

/// Runs `launcher guard-check` for the job of `call_dir` and `state_dir` with `env`, for
/// at most [`CHECK_LIMIT`] on `clock`.
pub(crate) async fn check(
    launcher: &Path,
    call_dir: &Path,
    state_dir: &Path,
    env: &SpawnEnv,
    clock: &Arc<dyn Clock>,
) -> Checked {
    let cwd = if state_dir.is_dir() { state_dir } else { Path::new("/") };
    let mut command = efr_stdx::process::command(launcher, cwd);
    command
        .env_clear()
        .env("PATH", &env.path)
        .env("HOME", &env.home)
        .env("XDG_RUNTIME_DIR", &env.user_runtime)
        .env("PWD", cwd)
        .arg(GUARD_CHECK)
        .arg("--call-dir")
        .arg(call_dir)
        .arg("--state-dir")
        .arg(state_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match clock.timeout(CHECK_LIMIT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            tracing::warn!(%error, state = %state_dir.display(), "the guard check of a job could not start");
            return Checked::Failed;
        }
        Err(_) => {
            tracing::warn!(state = %state_dir.display(), "the guard check of a job did not end in time");
            return Checked::Failed;
        }
    };
    match output.status.code() {
        Some(0) => Checked::Clean,
        Some(BROKEN) => {
            let dir = state_dir.to_path_buf();
            let report =
                tokio::task::spawn_blocking(move || read_report(&dir)).await.ok().flatten();
            match report {
                Some(report) if !report.changes.is_empty() => Checked::Broken(report),
                _ => {
                    tracing::warn!(state = %state_dir.display(), "a job's guard check found a change but wrote no report");
                    Checked::Failed
                }
            }
        }
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(state = %state_dir.display(), error = %stderr.trim(), "the guard check of a job failed");
            Checked::Failed
        }
    }
}

/// The `guard.json` in `state_dir`, if it reads.
fn read_report(state_dir: &Path) -> Option<GuardReport> {
    let path = state_dir.join(efr_sandbox::GUARD_FILE);
    let bytes = crate::sandbox::fs::DaemonFs::read(&path, MAX_REPORT_BYTES).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// True when two sets of roots share a path: one holds, or is, a root of the other.
pub(crate) fn overlap(first: &[PathBuf], second: &[PathBuf]) -> bool {
    first.iter().any(|one| second.iter().any(|two| one.starts_with(two) || two.starts_with(one)))
}

impl JobTable {
    /// Checks the git surface of each running contained job, or, with `roots`, of each
    /// whose guard roots overlap `roots`. A break stops the job with
    /// [`JobStopBy::Guard`]; the stop is done when this returns.
    pub(crate) async fn check_guards(&self, roots: Option<&[PathBuf]>) {
        let jobs: Vec<RunningJob> = self
            .running()
            .into_iter()
            // NOTE: a call that the user moved into a job has no state dir of a job: its
            // guard runs only at its end, in its launcher (DS25, OQS8).
            .filter(|job| job.contained && !job.moved)
            .filter(|job| roots.is_none_or(|roots| overlap(&job.guard_roots, roots)))
            .collect();
        for job in jobs {
            let inner = &self.inner;
            let checked = check(
                &inner.launcher,
                &job.call_dir,
                &job.state_dir,
                &inner.spawn_env,
                &inner.clock,
            )
            .await;
            if let Checked::Broken(report) = checked {
                self.guard_broke(job.key, report).await;
            }
        }
    }

    /// Keeps the changes of a guard break of the job `key` for its `job_ended`, and
    /// stops it.
    async fn guard_broke(&self, key: JobKey, report: GuardReport) {
        tracing::warn!(
            conversation = %key.conversation,
            job = key.number,
            changes = report.changes.len(),
            "a running job changed the git surface; it stops"
        );
        if let Some(entry) = self.lock().running.get_mut(&key) {
            for change in report.changes {
                if !entry.guard.contains(&change) {
                    entry.guard.push(change);
                }
            }
        }
        if let Err(error) = self.stop(key, JobStopBy::Guard).await {
            tracing::warn!(%error, job = key.number, "a job whose guard broke could not be stopped");
        }
    }

    /// Checks the git surface of every running contained job each
    /// [`GUARD_INTERVAL`] on efrd's clock, until the table's token ends.
    pub(crate) async fn guard_schedule(self) {
        let cancel = self.inner.cancel.clone();
        loop {
            tokio::select! {
                () = cancel.cancelled() => return,
                () = self.inner.clock.sleep(GUARD_INTERVAL) => {}
            }
            self.check_guards(None).await;
        }
    }

    /// The guard check of a contained job that the last efrd left with no
    /// `result.json`: its changes, which the check moved to the quarantine.
    pub(crate) async fn check_lost(&self, call_dir: PathBuf, state_dir: PathBuf) -> GuardReport {
        if !state_dir.is_dir() {
            return GuardReport::default();
        }
        let inner = &self.inner;
        let checked =
            check(&inner.launcher, &call_dir, &state_dir, &inner.spawn_env, &inner.clock).await;
        match checked {
            Checked::Broken(report) => report,
            Checked::Clean | Checked::Failed => GuardReport::default(),
        }
    }
}

#[cfg(test)]
mod tests;
