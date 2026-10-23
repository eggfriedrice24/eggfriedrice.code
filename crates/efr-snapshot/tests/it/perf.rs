//! The `bench_perf_*` benches of the snapshot store on fresh git repositories: the
//! snapshots of one call, the first snapshot of a turn and the end of a turn by the
//! number of tracked and changed files, with a large `target/` dir that is ignored or
//! not, and the turn end's left-out record by its size (the plan's W2-B). Ignored;
//! `just bench-perf` runs them in release.
//!
//! Each case reports its wall time on the plain git, and the git processes it started,
//! counted in a second store whose git is the logging script of `World::logged` (the
//! seam `efr_scope::Git::with_program`), on the same files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use efr_snapshot::{Limits, Root};
use efr_test_support::bench::Report;
use nix::sys::resource::{UsageWho, getrusage};

use crate::support::{World, conversation, git, git_root, turn, write};

/// A case that takes longer than this ends its ladder.
const STOP_AFTER: Duration = Duration::from_secs(10);

/// The tracked files of the call ladder. A root with more files than `max_files`
/// (20,000 by default) is not snapshotted, so the top of the ladder stays just below
/// it, and a root of 20,000 tracked files and its `.gitignore` shows the cap.
const TRACKED: [usize; 3] = [100, 2_000, 19_900];

/// The changed files of a call.
const CHANGED: [usize; 3] = [0, 1, 100];

/// The turns that each case of the call ladder is timed on.
const TURNS: u128 = 5;

/// The left-out paths of the turn-end ladder.
const LEFT_OUT: [usize; 4] = [300, 3_000, 10_000, 20_000];

/// The files of the `target/` dir.
const TARGET_FILES: usize = 10_000;

/// Writes `count` files of about 200 bytes below `dir/prefix`, in 100 dirs, from 2023,
/// as the files of a project that nobody changed lately: a file as new as the index is
/// racy for git, which then hashes it again.
fn write_files(dir: &Path, prefix: &str, count: usize) {
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for n in 0..count {
        let path = dir.join(format!("{prefix}/d{}/f{n}.rs", n % 100));
        write(&path, &format!("// file {n}\n{}\n", "fn body() -> u32 { 42 }\n".repeat(8)));
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
    }
}

/// A fresh git repository below the world's home with `count` tracked files in one
/// commit, and `ignore` as its `.gitignore`.
async fn repo(world: &World, name: &str, count: usize, ignore: &str) -> PathBuf {
    let home = world.home();
    let dir = world.dir(name);
    write_files(&dir, "src", count);
    write(&dir.join(".gitignore"), ignore);
    git(&dir, &home, &[], &["init", "-q"]).await;
    git(&dir, &home, &[], &["add", "-A"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "the files"]).await;
    dir
}

/// Changes `count` of the tracked files below `dir/src`; `round` makes the bytes new.
fn change(dir: &Path, count: usize, round: u128) {
    for n in 0..count {
        write(&dir.join(format!("src/d{}/f{n}.rs", n % 100)), &format!("// round {round}\n"));
    }
}

/// The CPU time of this process so far, its threads included and its children not.
fn cpu_self() -> Duration {
    let usage = getrusage(UsageWho::RUSAGE_SELF).unwrap();
    let time = |value: nix::sys::time::TimeVal| {
        Duration::from_secs(u64::try_from(value.tv_sec()).unwrap())
            + Duration::from_micros(u64::try_from(value.tv_usec()).unwrap())
    };
    time(usage.user_time()) + time(usage.system_time())
}

/// The git processes that the logged world started since the last look, and how many
/// of each subcommand.
fn runs(world: &World) -> (u64, BTreeMap<String, u64>) {
    let mut by = BTreeMap::new();
    let all = world.git_runs();
    for run in &all {
        *by.entry(run.clone()).or_insert(0) += 1;
    }
    (all.len() as u64, by)
}

/// Reports the git processes of `case` from the logged world, in all and by subcommand.
fn report_runs(report: &mut Report, world: &World, case: &str) {
    let (all, by) = runs(world);
    report.count(&format!("{case}: git processes"), all);
    let text: Vec<String> = by.iter().map(|(name, n)| format!("{name} {n}")).collect();
    // NOTE: the report has no text lines; the subcommands go into the label.
    report.count(&format!("{case}:   ({})", text.join(", ")), all);
}

/// The times of the steps of a turn on `roots`: the first snapshot (`pre` with the
/// ignored walk and the mirror), a call with each of [`CHANGED`] files changed (the
/// snapshot before and after it), and the turn's end.
#[derive(Default)]
struct TurnTimes {
    first: Vec<Duration>,
    calls: Vec<Vec<Duration>>,
    end: Vec<Duration>,
}

/// Runs one turn `id` in `world` on `dir`, and adds its times to `times`.
async fn one_turn(
    world: &World,
    dir: &Path,
    id: u128,
    roots: &dyn Fn() -> Vec<Root>,
    times: &mut TurnTimes,
) {
    let limits = Limits::default();
    let turn_id = turn(id);
    let started = Instant::now();
    let first = world.snapshots.before_call(conversation(1), turn_id, roots(), limits).await;
    times.first.push(started.elapsed());
    assert!(!first.is_empty());
    world.snapshots.after_call(first, limits).await;
    times.calls.resize(CHANGED.len(), Vec::new());
    for (at, count) in CHANGED.into_iter().enumerate() {
        let started = Instant::now();
        let call = world.snapshots.before_call(conversation(1), turn_id, roots(), limits).await;
        let before = started.elapsed();
        change(dir, count, id * 10 + at as u128);
        let started = Instant::now();
        let changes = world.snapshots.after_call(call, limits).await;
        times.calls[at].push(before + started.elapsed());
        assert_eq!(changes.is_some(), count > 0, "{count} changed files");
    }
    let started = Instant::now();
    let end = world.snapshots.turn_end(turn_id, limits).await;
    times.end.push(started.elapsed());
    assert!(end.kept, "{end:?}");
}

/// The same turn in the logged world, with the git processes of each step reported.
async fn counted_turn(
    report: &mut Report,
    world: &World,
    dir: &Path,
    id: u128,
    roots: &dyn Fn() -> Vec<Root>,
    name: &str,
) {
    let limits = Limits::default();
    let turn_id = turn(id);
    runs(world);
    let first = world.snapshots.before_call(conversation(1), turn_id, roots(), limits).await;
    report_runs(report, world, &format!("{name}: first snapshot of a turn"));
    world.snapshots.after_call(first, limits).await;
    runs(world);
    for (at, count) in CHANGED.into_iter().enumerate() {
        let call = world.snapshots.before_call(conversation(1), turn_id, roots(), limits).await;
        change(dir, count, id * 10 + at as u128);
        world.snapshots.after_call(call, limits).await;
        report_runs(report, world, &format!("{name}: one call, {count} changed"));
    }
    world.snapshots.turn_end(turn_id, limits).await;
    report_runs(report, world, &format!("{name}: turn end"));
}

/// Times [`TURNS`] turns of `dir` in `world` and counts the git processes of one more
/// in `logged`; true when a step took longer than [`STOP_AFTER`].
async fn ladder_step(
    report: &mut Report,
    world: &World,
    logged: &World,
    dir: &Path,
    name: &str,
    points: &mut Vec<(String, u64, Duration)>,
    files: u64,
) -> bool {
    let roots = || vec![git_root(dir)];
    let limits = Limits::default();
    // The first snapshot ever: every file hashed, the whole history fetched.
    let started = Instant::now();
    let first = world.snapshots.before_call(conversation(1), turn(1), roots(), limits).await;
    let skipped = first.is_empty();
    world.snapshots.after_call(first, limits).await;
    world.snapshots.turn_end(turn(1), limits).await;
    report.time(&format!("{name}: the first snapshot ever and its turn end"), &[started.elapsed()]);
    if skipped {
        report.count(&format!("{name}: root skipped, more files than max_files"), files);
        return true;
    }
    let mut times = TurnTimes::default();
    for id in 0..TURNS {
        one_turn(world, dir, 10 + id, &roots, &mut times).await;
    }
    let first =
        report.time(&format!("{name}: first snapshot of a turn (pre, mirror)"), &times.first);
    points.push(("first".to_owned(), files, first.p50));
    let mut slow = first.max > STOP_AFTER;
    for (at, count) in CHANGED.into_iter().enumerate() {
        let stats = report
            .time(&format!("{name}: one call, {count} changed (before + after)"), &times.calls[at]);
        points.push((format!("call {count}"), files, stats.p50));
        slow |= stats.max > STOP_AFTER;
    }
    let end = report.time(&format!("{name}: turn end"), &times.end);
    points.push(("end".to_owned(), files, end.p50));
    slow |= end.max > STOP_AFTER;

    let logged_roots = || vec![git_root(dir)];
    let first =
        logged.snapshots.before_call(conversation(1), turn(1), logged_roots(), limits).await;
    logged.snapshots.after_call(first, limits).await;
    logged.snapshots.turn_end(turn(1), limits).await;
    counted_turn(report, logged, dir, 90, &logged_roots, name).await;
    slow
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_snapshot_call() {
    let mut report = Report::new("bench_perf_snapshot_call");
    let mut points = Vec::new();
    for count in TRACKED {
        let world = World::in_target();
        let logged = World::in_target().logged();
        let started = Instant::now();
        let dir = repo(&world, "project", count, "target/\n").await;
        report.time(&format!("{count} files: fixture"), &[started.elapsed()]);
        let name = format!("{count} files");
        let slow =
            ladder_step(&mut report, &world, &logged, &dir, &name, &mut points, count as u64).await;
        if slow {
            report.count("stopped_at_n", count as u64);
            break;
        }
    }
    for step in ["first", "call 0", "call 1", "call 100", "end"] {
        let line: Vec<(u64, Duration)> = points
            .iter()
            .filter(|(name, _, _)| name == step)
            .map(|(_, files, time)| (*files, *time))
            .collect();
        report.slope(&format!("{step} over the tracked files"), &line);
    }

    // A build dir of 10,000 files beside 2,000 tracked files, ignored and not ignored.
    for (ignored, ignore) in [(true, "target/\n"), (false, "")] {
        let world = World::in_target();
        let logged = World::in_target().logged();
        let dir = repo(&world, "project", 2_000, ignore).await;
        write_files(&dir, "target/debug", TARGET_FILES);
        let name = if ignored { "target/ ignored" } else { "target/ not ignored" };
        let mut ignored_points = Vec::new();
        ladder_step(&mut report, &world, &logged, &dir, name, &mut ignored_points, 2_000).await;
    }

    // The cap: 20,000 tracked files and the `.gitignore`.
    let world = World::in_target();
    let dir = repo(&world, "project", 20_000, "target/\n").await;
    let call = world
        .snapshots
        .before_call(conversation(1), turn(1), vec![git_root(&dir)], Limits::default())
        .await;
    report.count("20000 files: roots that the snapshot skipped", u64::from(call.is_empty()));
    world.snapshots.after_call(call, Limits::default()).await;
    report.finish();
}

/// A repository with one tracked file in each of 200 dirs and `count` ignored files
/// spread over them, so `ls-files -o -i --directory` names each ignored file.
async fn scattered(world: &World, count: usize) -> PathBuf {
    let home = world.home();
    let dir = world.dir("scattered");
    for n in 0..200 {
        write(&dir.join(format!("d{n}/keep.rs")), "// kept\n");
    }
    write(&dir.join(".gitignore"), "*.log\n");
    git(&dir, &home, &[], &["init", "-q"]).await;
    git(&dir, &home, &[], &["add", "-A"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "the files"]).await;
    for n in 0..count {
        write(&dir.join(format!("d{}/run{n}.log", n % 200)), "log\n");
    }
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_left_out() {
    let mut report = Report::new("bench_perf_left_out");
    // NOTE: with the small ignored files off, each ignored file is left out of `pre`
    // and of `post`, so the turn end merges two records of `count` paths.
    let limits = Limits { ignored_small: false, ..Limits::default() };
    let mut walls = Vec::new();
    let mut cpus = Vec::new();
    for count in LEFT_OUT {
        let world = World::in_target();
        let dir = scattered(&world, count).await;
        let roots = || vec![git_root(&dir)];
        let (mut before, mut wall, mut cpu) = (Vec::new(), Vec::new(), Vec::new());
        let mut left = 0;
        for id in 0..3u128 {
            let turn_id = turn(10 + id);
            let started = Instant::now();
            let call = world.snapshots.before_call(conversation(1), turn_id, roots(), limits).await;
            before.push(started.elapsed());
            world.snapshots.after_call(call, limits).await;
            let (started, cpu_start) = (Instant::now(), cpu_self());
            let end = world.snapshots.turn_end(turn_id, limits).await;
            wall.push(started.elapsed());
            cpu.push(cpu_self().saturating_sub(cpu_start));
            left = end.left_out.len();
            if wall.last().is_some_and(|time| *time > STOP_AFTER) {
                break;
            }
        }
        assert_eq!(left, count, "every ignored file is left out once");
        report.count(&format!("{count} ignored: left-out paths of the turn"), left as u64);
        report.time(&format!("{count} ignored: first snapshot of a turn"), &before);
        let stats = report.time(&format!("{count} ignored: turn end, wall"), &wall);
        let cpu_stats =
            report.time(&format!("{count} ignored: turn end, cpu of efr's process"), &cpu);
        walls.push((count as u64, stats.p50));
        cpus.push((count as u64, cpu_stats.p50));
        if stats.max > STOP_AFTER {
            report.count("stopped_at_n", count as u64);
            break;
        }
    }
    report.slope("turn end wall over the left-out paths", &walls);
    let slope = report.slope("turn end cpu over the left-out paths", &cpus);
    report.finish();
    // The budget (plan W2-B): a seen-set beside the list makes the merge linear. The
    // linear `contains` of before gave a slope of 1.93 and 20.3 s at 20,000 paths.
    assert_eq!(walls.len(), LEFT_OUT.len(), "the ladder stopped early: {walls:?}");
    assert!(slope < 1.3, "the turn end grows faster than linear: slope {slope:.2}");
    let (most, wall) = walls[walls.len() - 1];
    assert!(wall < Duration::from_secs(1), "{most} left-out paths took {wall:?} at the turn end");
}
