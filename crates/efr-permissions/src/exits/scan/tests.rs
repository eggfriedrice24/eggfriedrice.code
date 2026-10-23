//! The bench of the lenient scan on deep nests of `$(` (`just bench-perf`, section 2.3
//! of the perf plan). A nest deep enough overflows the stack, which kills the whole
//! process, so each depth runs in a child process: this test binary again, with
//! [`CASE`] naming the depth, which runs [`scan_nesting_case`] only.

use std::io::{Read as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use efr_test_support::bench::Report;

use super::scan;

/// The variable that names the case of a child process, such as `closed:1000`.
const CASE: &str = "EFR_BENCH_SCAN_CASE";

/// The full name of the test that a child process runs.
const CHILD_TEST: &str = "exits::scan::tests::scan_nesting_case";

/// The stack of the thread that scans, as small as a tokio blocking thread's.
const STACK: usize = 2 * 1024 * 1024;

/// A depth that takes longer than this stops the ladder.
const LIMIT: Duration = Duration::from_secs(10);

/// How much longer than [`LIMIT`] a child process may run before it is killed.
const KILL_AFTER: Duration = Duration::from_secs(5);

/// The prefix of the lines that a child process reports.
const LINE: &str = "scan-case ";

/// `$(` `k` times around `x`, with the `k` closing parens or without them.
fn nested(k: usize, closed: bool) -> String {
    let mut line = "$(".repeat(k);
    line.push('x');
    if closed {
        line.push_str(&")".repeat(k));
    }
    line
}

/// What a child process gave.
enum Outcome {
    /// The scan returned: the time of each run and the count of segments.
    Done { samples: Vec<Duration>, segments: u64 },
    /// The process died, by a stack overflow or another signal.
    Crashed { overflow: bool, signal: Option<i32> },
    /// The process ran past [`LIMIT`] and [`KILL_AFTER`] and was killed.
    TimedOut,
}

#[expect(
    clippy::disallowed_methods,
    reason = "the bench runs its own test binary as a blocking child process"
)]
fn spawn(case: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([CHILD_TEST, "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env_clear()
        .env(CASE, case)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Runs `case` in a child process and reads what it reported.
fn child(case: &str) -> Outcome {
    let mut child = spawn(case);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > LIMIT + KILL_AFTER {
            let _ = child.kill();
            let _ = child.wait();
            return Outcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let (mut out, mut err) = (String::new(), String::new());
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    if !status.success() {
        return Outcome::Crashed {
            overflow: err.contains("overflowed its stack"),
            signal: status.signal(),
        };
    }
    let mut samples = Vec::new();
    let mut segments = 0;
    // NOTE: libtest writes the name of the test before the child's first line, on the
    // same line, so a report line may start anywhere in a line.
    for line in out.lines().filter_map(|line| line.find(LINE).map(|at| &line[at + LINE.len()..])) {
        if let Some(ns) = line.strip_prefix("sample_ns=") {
            samples.push(Duration::from_nanos(ns.parse().unwrap()));
        } else if let Some(count) = line.strip_prefix("segments=") {
            segments = count.parse().unwrap();
        }
    }
    assert!(!samples.is_empty(), "the child of {case} reported nothing:\n{out}\n{err}");
    Outcome::Done { samples, segments }
}

/// The child process of [`bench_perf_scan_nesting`]: scans the case of [`CASE`] on a
/// thread with a stack of [`STACK`] bytes and prints the time of each run. Without the
/// variable it does nothing.
#[test]
#[ignore = "bench: the child process of bench_perf_scan_nesting"]
fn scan_nesting_case() {
    let Some(case) = std::env::var_os(CASE) else {
        return;
    };
    let case = case.into_string().unwrap();
    let (kind, k) = case.split_once(':').unwrap();
    let line = nested(k.parse().unwrap(), kind == "closed");
    let (runs, segments) = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let mut runs = Vec::new();
            let mut segments = 0;
            let start = Instant::now();
            while runs.len() < 5 && (runs.is_empty() || start.elapsed() < Duration::from_secs(2)) {
                let one = Instant::now();
                segments = scan(&line).len();
                runs.push(one.elapsed());
            }
            (runs, segments)
        })
        .unwrap()
        .join()
        .unwrap();
    let mut out = std::io::stdout().lock();
    for run in runs {
        writeln!(out, "{LINE}sample_ns={}", run.as_nanos()).unwrap();
    }
    writeln!(out, "{LINE}segments={segments}").unwrap();
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_scan_nesting() {
    let mut report = Report::new("bench_perf_scan_nesting");
    for kind in ["closed", "unclosed"] {
        let mut points = Vec::new();
        for k in [100_u64, 1_000, 10_000, 50_000] {
            match child(&format!("{kind}:{k}")) {
                Outcome::Done { samples, segments } => {
                    let stats = report.time(&format!("{kind} $( x {k}"), &samples);
                    report.count(&format!("{kind} $( x {k}: segments"), segments);
                    points.push((k, stats.p50));
                    if stats.p50 > LIMIT {
                        report.count(&format!("{kind} stopped_at_n"), k);
                        break;
                    }
                }
                Outcome::Crashed { overflow, signal } => {
                    let why = if overflow { "stack overflow" } else { "crash" };
                    report.count(
                        &format!("{kind} $( x {k}: {why}, signal"),
                        signal.map_or(0, |signal| signal as u64),
                    );
                    report.count(&format!("{kind} crashed_at_k"), k);
                    break;
                }
                Outcome::TimedOut => {
                    report.count(&format!("{kind} $( x {k}: killed after 15 s"), 1);
                    report.count(&format!("{kind} stopped_at_n"), k);
                    break;
                }
            }
        }
        report.slope(&format!("{kind} slope over depth"), &points);
    }
    report.finish();
}
