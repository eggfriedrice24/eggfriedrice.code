//! The benches of the launcher on the real bwrap: the per-call launch of `true`, plain,
//! with the network proxy's bridge and with the reach helper, with the launcher's own
//! steps from `result.json`; and `efr-sbx guard-check` with 1, 5 and 10 git configs in
//! the guard roots, with the count of git runs from a counting `git` on the spec's
//! `PATH`: no run for a check when no config changed, one run after one changed.
//! Benches, ignored: `just bench-perf` runs them in release.
//!
//! They take the launcher from `EFR_TEST_SBX_BIN`, as the other tests do, or else a
//! private copy of the one that cargo built with this test binary
//! (`CARGO_BIN_EXE_efr-sbx`), so a build in the same target dir cannot change it while
//! a bench runs. Without a sandbox here, each one reports why and passes.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use efr_sandbox::JobPlan;
use efr_test_support::bench::Report;

use crate::support::{
    Fixture, ProxyAnswer, Ready, command, env_var, probe, run_with_timeout, target_tmp, which,
};

/// The calls of each launch series.
const CALLS: usize = 60;

/// The checks of each guard-check size.
const CHECKS: usize = 20;

/// The launcher for the benches, with the private dir of its copy when the bench made
/// one, or why they cannot run here.
fn launcher() -> Result<(Ready, Option<tempfile::TempDir>), String> {
    if let Some(bin) = env_var("EFR_TEST_SBX_BIN") {
        return probe(PathBuf::from(bin)).map(|ready| (ready, None));
    }
    let built = PathBuf::from(env!("CARGO_BIN_EXE_efr-sbx"));
    let dir = tempfile::Builder::new()
        .prefix("perf-bin")
        .tempdir_in(target_tmp())
        .map_err(|error| error.to_string())?;
    let copy = dir.path().join("efr-sbx");
    fs::copy(&built, &copy).map_err(|error| format!("copy {}: {error}", built.display()))?;
    probe(copy).map(|ready| (ready, Some(dir)))
}

/// The launcher and the dir of its copy, or `None` after a report line that says why
/// the bench skips.
fn ready_or_report(report: &mut Report) -> Option<(Ready, Option<tempfile::TempDir>)> {
    match launcher() {
        Ok(ready) => Some(ready),
        Err(reason) => {
            report.count(&format!("skipped: {reason}"), 0);
            None
        }
    }
}

/// The launcher's steps of one series: each phase with the time of each call.
type Steps = BTreeMap<String, Vec<Duration>>;

/// One call of `true`, from its call dir: the wall time and the launcher's steps.
fn launch(fixture: &Fixture, call_dir: &Path, steps: &mut Steps) -> Duration {
    let command = fixture.command(call_dir, &fixture.project);
    let start = Instant::now();
    let ended = run_with_timeout(command);
    let spent = start.elapsed();
    let run = fixture.collect(call_dir, ended);
    assert!(run.result().setup_error.is_none(), "{run:#?}");
    run.expect_status(0);
    for timing in &run.result().timings {
        steps.entry(timing.phase.clone()).or_default().push(Duration::from_micros(timing.us));
    }
    spent
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_launch() {
    let mut report = Report::new("bench_perf_launch");
    let Some((ready, _copy)) = ready_or_report(&mut report) else {
        report.finish();
        return;
    };
    let plain = Fixture::new(&ready);
    let mut proxied = Fixture::new(&ready);
    proxied.proxy(ProxyAnswer::Tunnel);
    let reach = Fixture::new(&ready);
    let names = ["plain", "proxy bridge", "reach"];
    let mut walls: [Vec<Duration>; 3] = Default::default();
    let mut steps: [Steps; 3] = Default::default();
    // NOTE: the series take turns, so the load of the machine weighs on each the same.
    for _ in 0..CALLS {
        let call_dir = plain.prepare("true");
        walls[0].push(launch(&plain, &call_dir, &mut steps[0]));
        let call_dir = proxied.prepare("true");
        walls[1].push(launch(&proxied, &call_dir, &mut steps[1]));
        let answer = Some(crate::reach::Answer::Echo);
        let (call_dir, listener) = crate::reach::prepare(&reach, "true", &[8765], answer);
        walls[2].push(launch(&reach, &call_dir, &mut steps[2]));
        drop(listener);
    }
    for ((name, wall), steps) in names.iter().zip(&walls).zip(&steps) {
        report.time(&format!("{name} call"), wall);
        for (phase, times) in steps {
            report.time(&format!("{name} launcher_{phase}"), times);
        }
    }
    report.finish();
}

/// A `git` that counts its runs in `<dir>/runs` and then runs the real git.
fn counting_git(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let real = which("git");
    let runs = dir.join("runs");
    let script = format!(
        "#!/bin/sh\nprintf 'x\\n' >> '{}'\nexec '{}' \"$@\"\n",
        runs.display(),
        real.display()
    );
    let git = dir.join("git");
    fs::write(&git, script).unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    runs
}

/// The runs of the counting git so far; the count starts at 0 again.
fn take_runs(runs: &Path) -> u64 {
    let count = fs::read(runs).map(|bytes| bytes.iter().filter(|b| **b == b'\n').count());
    let _ = fs::remove_file(runs);
    count.unwrap_or(0) as u64
}

/// Makes `configs` git repositories in the project, each with its own config.
fn repos(fixture: &Fixture, configs: usize) {
    for n in 0..configs {
        let repo = fixture.project.join(format!("repo-{n}"));
        fs::create_dir_all(&repo).unwrap();
        let status = command(which("git"))
            .args(["init", "-q"])
            .current_dir(&repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &fixture.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success());
        let config = repo.join(".git/config");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "[user]\n\tname = repo {n}\n[remote \"origin\"]\n\turl = /srv/{n}\n"
        ));
        fs::write(&config, text).unwrap();
    }
}

/// Runs `efr-sbx guard-check` for the job of `call_dir`, as efrd does: its status.
fn guard_check(fixture: &Fixture, call_dir: &Path) -> i32 {
    let call = call_dir.file_name().unwrap().to_string_lossy().parse().unwrap();
    let mut command = command(&fixture.ready.bin);
    command
        .args(["guard-check", "--call-dir"])
        .arg(call_dir)
        .arg("--state-dir")
        .arg(fixture.spec.runtime.job_state_dir(call))
        .current_dir("/")
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    run_with_timeout(command).0
}

/// A contained job of `true` that ran to its end: its call dir.
fn finished_job(fixture: &Fixture) -> PathBuf {
    let call_dir = fixture.prepare_job("true", JobPlan::new(600, 19));
    let mut job = fixture.spawn_job(&call_dir);
    job.start();
    job.wait(fixture).expect_status(0);
    call_dir
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_guard_check() {
    let mut report = Report::new("bench_perf_guard_check");
    let Some((ready, _copy)) = ready_or_report(&mut report) else {
        report.finish();
        return;
    };
    let mut points = Vec::new();
    let mut over = Vec::new();
    for configs in [1, 5, 10] {
        let mut fixture = Fixture::new(&ready);
        repos(&fixture, configs);
        // Two jobs that end at once: each leaves the record of its start in its state
        // dir, which is all that a check reads, with the git that its guard found. The
        // first has the real git, for the time; the second the counting git.
        let timed = finished_job(&fixture);
        let counting = fixture.base.join("counting-git");
        let runs = counting_git(&counting);
        // NOTE: a job's launcher takes the shell's PATH from `$CALL/env`, which comes
        // from the fixture's environment, and the guard runs the first git on it.
        let path = format!("{}:{}", counting.display(), fixture.spec.shell_path);
        fixture.spec.shell_path.clone_from(&path);
        fixture.env.insert("PATH".into(), path.into());
        let counted = finished_job(&fixture);
        let at_start = take_runs(&runs);
        let mut times = Vec::new();
        let mut per_check = Vec::new();
        for _ in 0..CHECKS {
            let start = Instant::now();
            let status = guard_check(&fixture, &timed);
            times.push(start.elapsed());
            assert_eq!(status, 0, "nothing changed");
            assert_eq!(guard_check(&fixture, &counted), 0, "nothing changed");
            per_check.push(take_runs(&runs));
        }
        // One config gets a key that breaks no rule: the next check lists it alone.
        let config = fixture.project.join("repo-0/.git/config");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str("[user]\n\temail = repo@example.invalid\n");
        fs::write(&config, text).unwrap();
        assert_eq!(guard_check(&fixture, &counted), 0, "no rule broken");
        let after_change = take_runs(&runs);
        let label = format!("{configs} configs");
        let stats = report.time(&format!("{label} guard-check"), &times);
        report.count(&format!("{label} git runs of the job's start and end"), at_start);
        let most = per_check.iter().max().copied().unwrap_or(0);
        report.count(&format!("{label} git runs per check"), most);
        report
            .count(&format!("{label} git runs of a check after one config changed"), after_change);
        // Budget: the job's state dir keeps its config listings, so a check runs git
        // only for a config that changed since the last one.
        if most > 0 || after_change > 1 {
            over.push(format!("{label}: {most} runs per check, {after_change} after a change"));
        }
        points.push((configs as u64, stats.p50));
    }
    let slope = report.slope("guard-check slope", &points);
    report.finish();
    assert!(over.is_empty(), "git runs over the budget: {over:?}");
    // NOTE: with a git run for each config the slope was 0.7; with none it is 0.1 to
    // 0.2 on a busy machine, because only the scan and the reads of the configs grow.
    assert!(slope < 0.5, "the cost of a check grows with its configs: slope {slope:.2}");
}
