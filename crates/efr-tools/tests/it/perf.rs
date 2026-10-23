//! The benches of the line diff and the patch engine (`just bench-perf`, section 2.3 of
//! the perf plan) on large files. The line diff has a budget on its counts and its
//! slope; the patch engine reports only.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use efr_protocol::MAX_CALL_DIFF_LINES;
use efr_test_support::bench::{self, Report};
use efr_tools::patch::{ChangeKind, apply, parse};
use efr_tools::written_diff;

/// A size of a ladder that takes longer than this stops the ladder.
const LIMIT: Duration = Duration::from_secs(10);

/// The measured runs of `f`: one run first, then up to four more while they fit in
/// about three seconds. The first run alone when it took longer than [`LIMIT`].
fn samples(mut f: impl FnMut()) -> Vec<Duration> {
    let start = Instant::now();
    f();
    let first = start.elapsed();
    let more = if first > LIMIT {
        0
    } else {
        (Duration::from_secs(3).as_nanos() / first.as_nanos().max(1)).min(4) as usize
    };
    let mut samples = vec![first];
    samples.extend(bench::run(0, more, f));
    samples
}

/// Line `at` of a text file: unique, about 60 bytes.
fn line(at: usize) -> String {
    format!("line {at:06}: let value = parse(&input, Kind::Item)?;\n")
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_line_diff() {
    let path = Path::new("/home/u/p/app/src/big.rs");
    let mut report = Report::new("bench_perf_line_diff");
    let mut points = Vec::new();
    let mut cases: Vec<(String, u64, String, String)> = Vec::new();
    for n in [3_000_usize, 10_000, 50_000] {
        let old: String = (0..n).map(line).collect();
        let new: String = (0..n)
            .map(|at| if at == 4 || at == n - 5 { format!("changed {at}\n") } else { line(at) })
            .collect();
        cases.push((format!("2 one-line changes in {n} lines"), n as u64, old, new));
    }
    let old: String = (0..50_000).map(line).collect();
    let new: String = (0..50_000).map(|at| format!("rewritten {at:06}\n")).collect();
    cases.push(("full rewrite of 50000 lines".to_owned(), 0, old, new));

    let mut failures = Vec::new();
    for (name, n, old, new) in &cases {
        let mut diff = None;
        let runs = samples(|| diff = written_diff(path, Some(old), new, MAX_CALL_DIFF_LINES));
        let diff = diff.unwrap();
        let stats = report.time(name, &runs);
        let lines = diff.text.lines().count();
        report.count(&format!("{name}: diff lines"), lines as u64);
        report.count(&format!("{name}: added"), diff.added as u64);
        report.count(&format!("{name}: removed"), diff.removed as u64);
        if *n > 0 {
            points.push((*n, stats.p50));
            // Budget: two hunks of 9 lines below the two header lines. Before the
            // anchors, the middle was one replacement of 2,003 lines at every size.
            if (diff.added, diff.removed) != (2, 2) || lines > 2 + 2 * 9 {
                failures.push(format!("{name}: {lines} lines, {diff:?}"));
            }
        }
    }
    let slope = report.slope("2 one-line changes slope over lines", &points);
    if slope > 1.3 {
        failures.push(format!("the slope over lines is {slope:.2}"));
    }
    report.finish();
    assert!(failures.is_empty(), "the budget failed: {failures:?}");
}

/// How the context of a patch differs from the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// Byte for byte: the exact pass matches.
    Exact,
    /// Each patch line has two more spaces at its end: the trailing space pass matches.
    TrailingSpace,
    /// The patch writes `'` where the file has `’`: only the last pass matches.
    Punctuation,
}

/// Line `at` of a 5 MB file of 50k lines: unique, about 100 bytes, with a typographic
/// apostrophe that a model often writes as `'`.
fn file_line(at: usize) -> String {
    format!(
        "line {at:06}: the parser\u{2019}s state stays here, with the plain text that fills this \
         line up to its end\n"
    )
}

/// The patch line for `line` of the file, as `context` writes it.
fn patch_line(line: &str, context: Context) -> String {
    let line = line.trim_end_matches('\n');
    match context {
        Context::Exact => line.to_owned(),
        Context::TrailingSpace => format!("{line}  "),
        Context::Punctuation => line.replace('\u{2019}', "'"),
    }
}

/// An update of `big.txt` of `lines` lines with `hunks` hunks spread over it, each one
/// changed line with three lines of context on each side.
fn big_patch(lines: usize, hunks: usize, context: Context) -> String {
    let mut text = String::from("*** Begin Patch\n*** Update File: big.txt\n");
    for hunk in 0..hunks {
        let at = (hunk + 1) * lines / (hunks + 1);
        text.push_str("@@\n");
        for near in at - 3..at {
            let _ = writeln!(text, " {}", patch_line(&file_line(near), context));
        }
        let _ = writeln!(text, "-{}", patch_line(&file_line(at), context));
        let _ = writeln!(text, "+changed line {at}");
        for near in at + 1..=at + 3 {
            let _ = writeln!(text, " {}", patch_line(&file_line(near), context));
        }
    }
    text.push_str("*** End Patch\n");
    text
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_patch_apply() {
    const LINES: usize = 50_000;
    let content: String = (0..LINES).map(file_line).collect();
    let files: BTreeMap<PathBuf, String> = [(PathBuf::from("big.txt"), content.clone())].into();
    let mut report = Report::new("bench_perf_patch_apply");
    report.bytes("file", content.len() as u64);
    for context in [Context::Exact, Context::TrailingSpace, Context::Punctuation] {
        let mut points = Vec::new();
        for hunks in [1_u64, 10, 100] {
            let text = big_patch(LINES, hunks as usize, context);
            let patch = parse(&text).unwrap();
            let mut changed = String::new();
            let runs = samples(|| {
                let changes = apply(&patch, &files).unwrap();
                match changes.into_iter().next().map(|change| change.kind) {
                    Some(ChangeKind::Updated { content }) => changed = content,
                    other => panic!("expected an update, got {other:?}"),
                }
            });
            let last = (hunks as usize) * LINES / (hunks as usize + 1);
            assert!(changed.contains(&format!("changed line {last}\n")), "{context:?} {hunks}");
            let stats = report.time(&format!("{context:?} context, {hunks} hunks"), &runs);
            points.push((hunks, stats.p50));
            if stats.p50 > LIMIT {
                report.count(&format!("{context:?} stopped_at_n"), hunks);
                break;
            }
        }
        report.slope(&format!("{context:?} slope over hunks"), &points);
    }
    let text = big_patch(LINES, 100, Context::Exact);
    let runs = samples(|| {
        std::hint::black_box(parse(&text).unwrap());
    });
    report.time("parse of the 100 hunk patch", &runs);
    report.finish();
}
