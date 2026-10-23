//! The benches of the renderer (`just bench-perf`, section 2.3 of the perf plan): a
//! large fenced diff rendered at once, and a long fence inside a list item that streams
//! in small pieces. They report; they have no time budget yet.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use efr_render::{RenderOptions, Renderer, render};
use efr_test_support::bench::{self, Report};

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

/// A fenced `diff` block of `lines` diff lines of Rust code: kept, removed and added
/// lines in turn, after the headers of `src/lib.rs`, so the diff's file gets its syntax
/// colours.
fn rust_diff(lines: usize) -> String {
    let mut text = String::from("```diff\n--- a/src/lib.rs\n+++ b/src/lib.rs\n");
    let _ = writeln!(text, "@@ -1,{lines} +1,{lines} @@");
    for at in 0..lines {
        let mark = [' ', '-', '+'][at % 3];
        let _ = writeln!(
            text,
            "{mark}    let value_{at} = parse(&input[{at}..], Kind::Item)?; // {at}"
        );
    }
    text.push_str("```\n");
    text
}

/// A fenced `diff` block of a lockfile: `lines` lines of about `width` bytes each, as
/// a minified lock or an integrity list has them.
fn lockfile_diff(lines: usize, width: usize) -> String {
    let mut text = String::from("```diff\n--- a/package-lock.json\n+++ b/package-lock.json\n");
    let _ = writeln!(text, "@@ -1,{lines} +1,{lines} @@");
    let hash: String = "Zm9vYmFyYmF6cXV4".chars().cycle().take(width.saturating_sub(60)).collect();
    for at in 0..lines {
        let mark = if at % 2 == 0 { '-' } else { '+' };
        let _ = writeln!(
            text,
            "{mark}  \"node_modules/p{at}\": {{ \"integrity\": \"sha512-{hash}\" }},"
        );
    }
    text.push_str("```\n");
    text
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_render_diff() {
    let options = RenderOptions::new(80);
    let mut report = Report::new("bench_perf_render_diff");
    let mut points = Vec::new();
    for lines in [2_000, 10_000, 20_000] {
        let markdown = rust_diff(lines);
        let mut rows = 0;
        let runs = samples(|| rows = render(&markdown, &options).lines().count());
        assert!(rows >= lines, "{rows} rows for {lines} diff lines");
        let stats = report.time(&format!("rust diff {lines} lines"), &runs);
        points.push((lines as u64, stats.p50));
        if stats.p50 > LIMIT {
            report.count("stopped_at_n", lines as u64);
            break;
        }
    }
    report.slope("rust diff slope over lines", &points);
    let markdown = lockfile_diff(2_000, 1_000);
    report.bytes("lockfile diff markdown", markdown.len() as u64);
    let mut rows = 0;
    let runs = samples(|| rows = render(&markdown, &options).lines().count());
    assert!(rows >= 2_000, "{rows} rows for the lockfile diff");
    report.time("lockfile diff 2k lines of 1 KB", &runs);
    report.finish();
}

/// A `1.` list item that holds a Rust fence of `lines` lines, as a model writes a step
/// of a plan with its code.
fn nested_fence(lines: usize) -> String {
    let mut text = String::from("Here is the change:\n\n1. Change the parser:\n\n   ```rust\n");
    for at in 0..lines {
        let _ = writeln!(text, "   let value_{at} = parse(&input[{at}..], Kind::Item)?;");
    }
    text.push_str("   ```\n\n2. Run the tests.\n");
    text
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_live_nested_fence() {
    const CHUNK: usize = 16;
    let options = RenderOptions::new(80);
    let mut report = Report::new("bench_perf_live_nested_fence");
    let mut points = Vec::new();
    for lines in [30, 150, 300, 1_000] {
        let markdown = nested_fence(lines);
        let mut renderer = Renderer::new(options.clone());
        let mut pushes = Vec::new();
        let mut out = String::new();
        let start = Instant::now();
        let mut stopped = false;
        let mut at = 0;
        while at < markdown.len() {
            let mut end = (at + CHUNK).min(markdown.len());
            while !markdown.is_char_boundary(end) {
                end += 1;
            }
            let push = Instant::now();
            let update = renderer.push(&markdown[at..end]);
            pushes.push(push.elapsed());
            out.push_str(update.committed());
            at = end;
            if start.elapsed() > LIMIT {
                stopped = true;
                break;
            }
        }
        out.push_str(&renderer.finish());
        let total = start.elapsed();
        report.time(&format!("{lines} lines: total"), &[total]);
        report.time(&format!("{lines} lines: per push"), &pushes);
        report.count(&format!("{lines} lines: pushes"), pushes.len() as u64);
        if stopped {
            report.count("stopped_at_n", lines as u64);
            break;
        }
        assert!(out.contains("value_0"), "the fence was not committed");
        points.push((lines as u64, total));
    }
    report.slope("total slope over lines", &points);
    report.finish();
}
