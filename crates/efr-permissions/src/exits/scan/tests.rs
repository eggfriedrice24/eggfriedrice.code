//! The depth cap of the lenient scan, and its bench on deep nests of `$(`
//! (`just bench-perf`, section 2.3 of the perf plan). Before the cap a nest deep
//! enough overflowed the stack, which kills the whole process, so each depth of the
//! bench runs in a child process: this test binary again, with [`CASE`] naming the
//! depth, which runs [`scan_nesting_case`] only.

use std::io::{Read as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use efr_test_support::bench::Report;
use proptest::prelude::*;

use super::{MAX_DEPTH, OPAQUE, Segment, scan, scan_to};

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

/// The segments of `segments` that stand for a substitution that the split did not
/// read.
fn unread(segments: &[Segment]) -> Vec<&Segment> {
    segments.iter().filter(|segment| segment.too_deep).collect()
}

/// The split of `line` on a thread with a stack of [`STACK`] bytes.
fn scan_on_small_stack(line: String) -> Vec<Segment> {
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || scan(&line))
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn the_split_reads_substitutions_down_to_the_cap() {
    let at_cap = nested(MAX_DEPTH, true);
    let segments = scan(&at_cap);
    assert!(unread(&segments).is_empty(), "{segments:?}");
    assert_eq!(segments, scan_to(&at_cap, usize::MAX));
    assert_eq!(segments[0].words, ["x"]);
    let deletes = format!("{}`rm -rf ~`{}", "$(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
    for line in [nested(MAX_DEPTH + 1, true), deletes] {
        let segments = scan(&line);
        let unread = unread(&segments);
        assert_eq!(unread.len(), 1, "{segments:?}");
        assert_eq!(unread[0].words, ["sh", "-c", &OPAQUE.to_string()]);
        // NOTE: the text below the cap is not read: no `x` and no `rm` comes out.
        let read = segments.iter().filter(|segment| !segment.too_deep);
        assert!(read.flat_map(|segment| &segment.words).all(|word| word.contains(OPAQUE)));
    }
}

#[test]
fn a_nest_far_past_the_cap_returns_on_a_small_stack() {
    for closed in [true, false] {
        for k in [10_000, 50_000] {
            let segments = scan_on_small_stack(nested(k, closed));
            assert_eq!(unread(&segments).len(), 1, "closed {closed}, {k} deep");
            // NOTE: one segment for each level that the split reads, the line and
            // `MAX_DEPTH` substitutions, and the one that stands for the rest.
            assert_eq!(segments.len(), MAX_DEPTH + 2, "closed {closed}, {k} deep");
        }
    }
}

/// A random piece of a line: words, operators, quotes, expansions and the openings and
/// closings of substitutions.
fn piece() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "$(",
        "$(",
        ")",
        "`",
        "<(",
        ">(",
        "=(",
        "\"",
        "'",
        "$'a'",
        "${a}",
        "$b",
        "\\",
        " ",
        "\n",
        ";",
        "|",
        "&&",
        "&",
        "> out",
        "2>&1",
        "<<E\nx\nE\n",
        "echo",
        "rm -rf",
        "cd d",
        "x",
        "#",
        "{",
        "}",
        "(",
    ])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// A line that nests no deeper than the cap gives the same segments as the split
    /// without a cap.
    #[test]
    fn a_line_below_the_cap_splits_as_without_a_cap(
        depth in 0..MAX_DEPTH - 24,
        pieces in prop::collection::vec(piece(), 0..24),
        closed in any::<bool>(),
    ) {
        let mut line = "$(".repeat(depth);
        line.push_str(&pieces.concat());
        if closed {
            line.push_str(&")".repeat(depth));
        }
        let segments = scan(&line);
        prop_assert!(unread(&segments).is_empty(), "{:?}", line);
        prop_assert_eq!(segments, scan_to(&line, usize::MAX), "{:?}", line);
    }
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
    // NOTE: the budget: every depth returns, with the segments of the cap, and the
    // time grows no more than linearly.
    let mut failed: Vec<String> = Vec::new();
    for kind in ["closed", "unclosed"] {
        let mut points = Vec::new();
        for k in [100_u64, 1_000, 10_000, 50_000] {
            match child(&format!("{kind}:{k}")) {
                Outcome::Done { samples, segments } => {
                    let stats = report.time(&format!("{kind} $( x {k}"), &samples);
                    report.count(&format!("{kind} $( x {k}: segments"), segments);
                    points.push((k, stats.p50));
                    // NOTE: past the cap the split reads the line and `MAX_DEPTH`
                    // substitutions, and one segment stands for the rest.
                    let read =
                        (k as usize).min(MAX_DEPTH) + 1 + usize::from(k as usize > MAX_DEPTH);
                    if segments != read as u64 {
                        failed.push(format!("{kind} $( x {k}: {segments} segments, not {read}"));
                    }
                    if stats.p50 > LIMIT {
                        report.count(&format!("{kind} stopped_at_n"), k);
                        failed.push(format!("{kind} $( x {k} stopped the ladder"));
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
                    failed.push(format!("{kind} $( x {k}: {why}"));
                    break;
                }
                Outcome::TimedOut => {
                    report.count(&format!("{kind} $( x {k}: killed after 15 s"), 1);
                    report.count(&format!("{kind} stopped_at_n"), k);
                    failed.push(format!("{kind} $( x {k} was killed"));
                    break;
                }
            }
        }
        let slope = report.slope(&format!("{kind} slope over depth"), &points);
        // NOTE: each of the levels that the split reads scans the rest of the line once,
        // so the time is linear in the line; it was quadratic with no cap.
        if slope >= 1.3 {
            failed.push(format!("{kind}: slope {slope:.2} over depth, the budget is 1.3"));
        }
    }
    report.finish();
    assert!(failed.is_empty(), "{failed:#?}");
}
