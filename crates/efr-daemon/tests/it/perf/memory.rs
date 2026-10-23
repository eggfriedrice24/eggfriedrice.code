//! `bench_perf_daemon_memory` and `bench_perf_daemon_growth`: the resident memory of
//! the daemon. The daemon of a `TestDaemon` runs in this test's process, so the numbers
//! are those of this process (`VmRSS` and `VmHWM` from `/proc/self/status`): they hold
//! the test's client and its runtime too, which a bench starts before the first line,
//! so the differences between the lines are the daemon's.
//!
//! Each turn reads a file with `read_file` and answers. The memory bench reads a file of
//! 1 MiB, which `read_file` cuts to its output limit of 32 KiB, so the history of each
//! turn holds 32 KiB of tool output; the model's context window makes the longer
//! conversations compact. The growth bench reads a file of 4 KiB.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_provider::Provider;
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::TestDaemon;
use efr_test_support::TestClock;
use efr_test_support::bench::{self, Memory, Report};
use serde_json::json;

use super::{Follower, PerfModel, Plan, builder, prompt, run_turn, tree};

/// The idle period that the memory bench moves the clock by: past the idle collector of
/// the hidden shells (60 minutes) and the debounce of the indexer.
const IDLE: Duration = Duration::from_secs(3 * 3600);

/// The real time that the bench waits after it moved the clock, so the timers run.
const SETTLE: Duration = Duration::from_secs(3);

/// The memory of this process now.
fn memory() -> Memory {
    Memory::of_self().expect("/proc/self/status")
}

/// `turns` turns in the conversation of the terminal `tty`, each one read through
/// `client`; stops early at `deadline`. Gives the turns that ran.
async fn turns(
    daemon: &TestDaemon,
    tty: &str,
    cwd: &Path,
    turns: usize,
    deadline: Instant,
) -> usize {
    let client = daemon.client_for_tty(tty).await.unwrap();
    for n in 0..turns {
        if Instant::now() > deadline {
            return n;
        }
        let run = run_turn(&client, prompt(tty, cwd, "read the file", None)).await;
        assert_eq!(run.end_kind(), "turn_completed", "{:#?}", run.events);
    }
    turns
}

/// A daemon whose model reads `file` of `bytes` in each turn, on a new tree.
async fn reading_daemon(bytes: usize) -> (TestDaemon, TestClock, Follower, std::path::PathBuf) {
    let dirs = tree();
    let cwd = dirs.create_dir("home/project").unwrap();
    std::fs::write(
        cwd.join("file.txt"),
        "a line of the file that the model reads\n".repeat(bytes / 40),
    )
    .unwrap();
    let clock = TestClock::new();
    let follower = Follower::start(&clock);
    let model =
        PerfModel::new(Plan::call("read_file", json!({ "path": "file.txt" }), "I read it."));
    let daemon = Box::pin(
        builder(&dirs, &clock)
            .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
            .config(|config| config.shell.idle_minutes = 60)
            .start(),
    )
    .await
    .unwrap();
    (daemon, clock, follower, cwd)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_daemon_memory() {
    const CONVERSATIONS: usize = 20;
    const TURNS: usize = 50;
    let mut report = Report::new("bench_perf_daemon_memory");
    let (daemon, clock, _follower, cwd) = reading_daemon(1024 * 1024).await;
    // NOTE: one turn first, so the client's side and the daemon's first-use costs are
    // in the line after the start.
    let first = turns(&daemon, "/dev/pts/perf-memory-first", &cwd, 1, Instant::now() + IDLE).await;
    assert_eq!(first, 1);
    bench::reset_peak();
    let start = memory();
    report.bytes("after start: rss", start.rss);
    let begin = Instant::now();
    let deadline = begin + Duration::from_secs(90);
    let ran = futures::future::join_all((0..CONVERSATIONS).map(|n| {
        let tty = format!("/dev/pts/perf-memory-{n}");
        let cwd = cwd.clone();
        let daemon = &daemon;
        async move { turns(daemon, &tty, &cwd, TURNS, deadline).await }
    }))
    .await;
    let done: usize = ran.iter().sum();
    report.count("turns of the load", done as u64);
    if done < CONVERSATIONS * TURNS {
        report.count("stopped_at_n (turns, 90 s)", done as u64);
    }
    report.time("load", &[begin.elapsed()]);
    let loaded = memory();
    report.bytes("after the load: rss", loaded.rss);
    report.bytes("after the load: peak (hwm)", loaded.peak);
    report.bytes("the load added: rss", loaded.rss.saturating_sub(start.rss));
    clock.advance(IDLE);
    SystemClock.sleep(SETTLE).await;
    let idle = memory();
    report.bytes("after the idle period: rss", idle.rss);
    report.bytes("the idle period freed: rss", loaded.rss.saturating_sub(idle.rss));
    daemon.stop().await.unwrap();
    report.finish();
}

/// The least squares slope of `points`, in bytes per conversation.
fn bytes_per(points: &[(u64, u64)]) -> u64 {
    let count = points.len() as f64;
    let mean_x = points.iter().map(|(x, _)| *x as f64).sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| *y as f64).sum::<f64>() / count;
    let covariance: f64 =
        points.iter().map(|(x, y)| (*x as f64 - mean_x) * (*y as f64 - mean_y)).sum();
    let variance: f64 = points.iter().map(|(x, _)| (*x as f64 - mean_x).powi(2)).sum();
    if variance == 0.0 { 0 } else { (covariance / variance).max(0.0) as u64 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_daemon_growth() {
    const CONVERSATIONS: usize = 300;
    const BATCH: usize = 50;
    const TURNS: usize = 5;
    /// The conversations that run at one time.
    const AT_ONCE: usize = 10;
    let mut report = Report::new("bench_perf_daemon_growth");
    let (daemon, clock, _follower, cwd) = reading_daemon(4 * 1024).await;
    let first = turns(&daemon, "/dev/pts/perf-growth-first", &cwd, 1, Instant::now() + IDLE).await;
    assert_eq!(first, 1);
    let start = memory();
    report.bytes("after start: rss", start.rss);
    let mut points = vec![(0, start.rss)];
    let mut times = Vec::new();
    let mut opened = 0;
    while opened < CONVERSATIONS {
        let begin = Instant::now();
        let deadline = begin + Duration::from_secs(60);
        for chunk in (opened..opened + BATCH).collect::<Vec<_>>().chunks(AT_ONCE) {
            futures::future::join_all(chunk.iter().map(|n| {
                let tty = format!("/dev/pts/perf-growth-{n}");
                let cwd = cwd.clone();
                let daemon = &daemon;
                async move { turns(daemon, &tty, &cwd, TURNS, deadline).await }
            }))
            .await;
        }
        opened += BATCH;
        let time = begin.elapsed();
        times.push(time);
        let now = memory();
        points.push((opened as u64, now.rss));
        if [50, 150, 300].contains(&opened) {
            report.bytes(&format!("after {opened} conversations: rss"), now.rss);
        }
        if time > Duration::from_secs(60) {
            report.count("stopped_at_n (conversations)", opened as u64);
            break;
        }
    }
    report.time(&format!("{BATCH} conversations x {TURNS} turns"), &times);
    report.bytes("rss growth per conversation (fit)", bytes_per(&points[1..]));
    report.count("peak (hwm) in MiB", memory().peak / (1024 * 1024));
    clock.advance(IDLE);
    SystemClock.sleep(SETTLE).await;
    report.bytes("after the idle period: rss", memory().rss);
    daemon.stop().await.unwrap();
    report.finish();
}
