//! The benches of the renderer (`just bench-perf`, section 2.3 of the perf plan): a
//! large fenced diff rendered at once, and a long fence inside a list item that streams
//! in small pieces. Their budgets are slopes and ratios, not times: `render` grows
//! with the length of the text, and a push in an open fence costs the same at 30 and
//! at 1,000 lines.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use efr_render::{RenderOptions, Renderer, render};
use efr_test_support::bench::{self, Report};

/// A size of a ladder that takes longer than this stops the ladder.
const LIMIT: Duration = Duration::from_secs(10);

/// The highest slope of a cost that must grow with the size and no faster.
const LINEAR: f64 = 1.3;

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

/// A fenced `text` block of `lines` lines: no syntax colours, so the time is the
/// renderer's own and not the highlighter's.
fn text_fence(lines: usize) -> String {
    let mut text = String::from("```text\n");
    for at in 0..lines {
        let _ = writeln!(text, "line {at} of a long log that a tool printed");
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
    let diff_slope = report.slope("rust diff slope over lines", &points);
    let markdown = lockfile_diff(2_000, 1_000);
    report.bytes("lockfile diff markdown", markdown.len() as u64);
    let mut rows = 0;
    let runs = samples(|| rows = render(&markdown, &options).lines().count());
    assert!(rows >= 2_000, "{rows} rows for the lockfile diff");
    report.time("lockfile diff 2k lines of 1 KB", &runs);
    let mut points = Vec::new();
    let mut stopped = false;
    for lines in [20_000, 50_000, 100_000] {
        let markdown = text_fence(lines);
        let mut rows = 0;
        let runs = samples(|| rows = render(&markdown, &options).lines().count());
        assert_eq!(rows, lines, "the rows of the text fence");
        let stats = report.time(&format!("text fence {lines} lines"), &runs);
        points.push((lines as u64, stats.p50));
        if stats.p50 > LIMIT {
            report.count("text fence: stopped_at_n", lines as u64);
            stopped = true;
            break;
        }
    }
    let text_slope = report.slope("text fence slope over lines", &points);
    report.finish();
    assert!(diff_slope < LINEAR, "the rust diff grows with slope {diff_slope:.2}");
    assert!(!stopped, "a text fence took more than {LIMIT:?}");
    assert!(text_slope < LINEAR, "the text fence grows with slope {text_slope:.2}");
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
    // The grammars load before the first size, so its pushes are not cold.
    render("```rust\nlet x = 1;\n```\n", &options);
    let mut points = Vec::new();
    let mut per_push = Vec::new();
    let mut stopped = false;
    for lines in [30, 150, 300, 1_000] {
        let markdown = nested_fence(lines);
        let mut renderer = Renderer::new(options.clone());
        let mut pushes = Vec::new();
        let mut out = String::new();
        let start = Instant::now();
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
        let stats = report.time(&format!("{lines} lines: per push"), &pushes);
        report.count(&format!("{lines} lines: pushes"), pushes.len() as u64);
        if stopped {
            report.count("stopped_at_n", lines as u64);
            break;
        }
        assert!(out.contains("value_0"), "the fence was not committed");
        points.push((lines as u64, total));
        per_push.push((lines, stats.p50));
    }
    let slope = report.slope("total slope over lines", &points);
    report.finish();
    assert!(!stopped, "a size took more than {LIMIT:?}");
    assert!(slope < LINEAR, "the total time grows with slope {slope:.2}");
    // A push costs the same at every size: its median at 1,000 lines stays
    // within three times the one at 30 lines. Before, it was eight times at 300 lines.
    let (first, last) = (per_push[0], per_push[per_push.len() - 1]);
    assert!(
        last.1 <= first.1 * 3,
        "a push takes {:?} at {} lines and {:?} at {} lines",
        last.1,
        last.0,
        first.1,
        first.0
    );
}
