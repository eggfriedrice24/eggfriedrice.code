//! `bench_perf_stream_ladder`: what one frame of the view costs and writes while the
//! open last block of the message has a given size, from 256 bytes to 64 KiB. The
//! drafts and the frames come one after another without a pause, so the CPU runs at
//! its full speed; the paced runs are in `follow/tests/stream.rs`.
//!
//! Each kind of block (a paragraph, a table, a fenced block in a list item that is
//! still open or closed, and others) runs at 80x24 and 200x60. A rung whose runs take
//! more than 10 s ends the ladder of its kind and screen. The lines: the CPU time of a
//! frame (p50, p95, max), its bytes, the CPU time of the first frame, and the slope of
//! the p50 over the size. `EFR_STREAM_KINDS` names the kinds of a run by hand.
//! Budget: no streamed frame takes 16 ms of CPU; for a fenced block in a list item, no
//! frame takes 2 ms, and the bytes of a frame do not grow with the block.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use efr_render::{ColourMode, RenderOptions, WidthMethod};
use efr_test_support::bench::Report;

use super::super::{Look, TurnView};
use super::{at, sent, text_draft, turn_started};
use crate::terminal::Size;
use crate::testing::probe::thread_cpu;
use crate::testing::turn;

/// The frames that are measured at each size, each after a draft of [`DELTA`] bytes.
const FRAMES: usize = 40;
const DELTA: usize = 4;
/// How often each size is run; the samples of all runs count.
const RUNS: usize = 5;

fn words(text: &mut String, bytes: usize) {
    let start = text.len();
    let mut n = 0;
    while text.len() - start < bytes {
        let _ = write!(text, "{} ", ["alpha", "bravo", "delta", "gamma", "omega"][n % 5]);
        n += 1;
    }
}

/// A text whose last block is of kind `kind`, is still open and is about `bytes` bytes
/// long, without a newline at its end.
fn open_block(kind: &str, bytes: usize) -> String {
    let mut text = String::new();
    match kind {
        // One source line of prose.
        "paragraph" => words(&mut text, bytes),
        // Lines of 72 characters: all but the last three commit.
        "hard_breaks" => {
            while text.len() < bytes {
                words(&mut text, 72);
                text.push('\n');
            }
        }
        "table" => {
            text.push_str("| one | two | three | four |\n|---|---|---|---|\n");
            while text.len() < bytes {
                text.push_str("| alpha bravo | delta gamma | omega alpha | bravo delta |\n");
            }
        }
        "quote" => {
            while text.len() < bytes {
                text.push_str("> ");
                words(&mut text, 72);
                text.push('\n');
            }
        }
        // One item of a list with the other items below it.
        "nested_list" => {
            text.push_str("- the first item of the list\n");
            while text.len() < bytes {
                text.push_str("  - ");
                words(&mut text, 55);
                text.push('\n');
            }
        }
        // Items of the top level: all but the last commit.
        "list" => {
            while text.len() < bytes {
                text.push_str("- ");
                words(&mut text, 55);
                text.push('\n');
            }
        }
        // One long item, which the renderer wraps.
        "long_item" => {
            text.push_str("- ");
            words(&mut text, bytes);
        }
        "fence" => {
            text.push_str("```rust\n");
            while text.len() < bytes {
                text.push_str("    let alpha = bravo.delta(gamma, 42);\n");
            }
        }
        "fence_in_list" => {
            text.push_str("1. the first step\n\n   ```rust\n");
            while text.len() < bytes {
                text.push_str("       let alpha = bravo.delta(gamma, 42);\n");
            }
        }
        // A fenced block in a list item that is closed, then the start of the next
        // item on a line that is not complete: the frames fall into that line.
        "closed_fence_in_list" => {
            text.push_str("1. the first step\n\n   ```rust\n");
            while text.len() + 240 < bytes {
                text.push_str("       let alpha = bravo.delta(gamma, 42);\n");
            }
            text.push_str("   ```\n2. ");
            words(&mut text, 240);
        }
        other => panic!("no kind {other}"),
    }
    text.truncate(bytes);
    while text.ends_with('\n') || text.ends_with(' ') {
        text.pop();
    }
    text
}

/// A rung whose runs take longer than this ends its ladder.
const STOP_AFTER: Duration = Duration::from_secs(10);

/// The bench reads its own switch.
#[expect(clippy::disallowed_methods, reason = "the bench reads its own switch")]
fn kinds_var() -> Option<String> {
    std::env::var("EFR_STREAM_KINDS").ok().filter(|value| !value.is_empty())
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_stream_ladder() {
    let mut report = Report::new("bench_perf_stream_ladder");
    let kinds = [
        "paragraph",
        "hard_breaks",
        "table",
        "quote",
        "nested_list",
        "list",
        "long_item",
        "fence",
        "fence_in_list",
        "closed_fence_in_list",
    ];
    let only = kinds_var();
    let kinds = kinds
        .into_iter()
        .filter(|kind| only.as_ref().is_none_or(|only| only.split(',').any(|name| name == *kind)));
    let screens = [Size { cols: 80, rows: 24 }, Size { cols: 200, rows: 60 }];
    let mut slowest: Option<(Duration, String)> = None;
    // For a fenced block in a list item: the largest frame of each rung, in bytes, and
    // the CPU time of its slowest frame.
    let mut fences: Vec<(String, usize, usize, Duration)> = Vec::new();
    for kind in kinds {
        for size in screens {
            let case = format!("{kind}|{}x{}", size.cols, size.rows);
            let mut points = Vec::new();
            for bytes in [256, 512, 1024, 2048, 4000, 8192, 16_384, 32_768, 65_536] {
                let text = open_block(kind, bytes);
                let head = text.len().saturating_sub(FRAMES * DELTA);
                let mut cpu = Vec::new();
                let mut frame_bytes = Vec::new();
                let mut first_cpu = Vec::new();
                let began = Instant::now();
                for _ in 0..RUNS {
                    // As on the user's terminal: true colour and widths by grapheme.
                    let options = RenderOptions::new(size.cols)
                        .with_colour(ColourMode::TrueColor)
                        .with_width_method(WidthMethod::Grapheme);
                    let look =
                        Look { motion: true, summary: true, progress: false, diff_lines: 20 };
                    let mut view = TurnView::new(turn(), options).with_look(look).with_input();
                    view.start();
                    view.show_row(true);
                    view.envelope(&sent(11, 0, turn_started()), size, false);
                    view.draft(&text_draft(11, 0, &text[..head]), size);
                    // The first frame renders the whole head at once.
                    let start = thread_cpu();
                    let _ = view.frame(size, at(0));
                    first_cpu.push(thread_cpu().saturating_sub(start));
                    let mut offset = head;
                    let mut millis = 17;
                    while offset < text.len() {
                        let end = (offset + DELTA).min(text.len());
                        view.draft(&text_draft(11, offset, &text[offset..end]), size);
                        let start = thread_cpu();
                        let frame = view.frame(size, at(millis));
                        cpu.push(thread_cpu().saturating_sub(start));
                        frame_bytes.push(frame.len());
                        offset = end;
                        millis += 17;
                    }
                }
                let rung = format!("{case}|{bytes} bytes pending");
                if let Some(max) = cpu.iter().max()
                    && slowest.as_ref().is_none_or(|(slowest, _)| max > slowest)
                {
                    slowest = Some((*max, rung.clone()));
                }
                let stats = report.time(&format!("{rung}|cpu of a frame"), &cpu);
                points.push((bytes as u64, stats.p50));
                report.time(&format!("{rung}|cpu of the first frame"), &first_cpu);
                frame_bytes.sort_unstable();
                let p50 = frame_bytes.get(frame_bytes.len() / 2).copied().unwrap_or(0);
                let max = frame_bytes.last().copied().unwrap_or(0);
                report.count(&format!("{rung}|bytes of a frame, p50"), p50 as u64);
                report.count(&format!("{rung}|bytes of a frame, max"), max as u64);
                if kind.contains("fence_in_list") {
                    let slowest = cpu.iter().max().copied().unwrap_or_default();
                    fences.push((rung.clone(), bytes, max, slowest));
                }
                if began.elapsed() > STOP_AFTER {
                    report.count(&format!("{case}|stopped_at_n (bytes)"), bytes as u64);
                    break;
                }
            }
            report.slope(&format!("{case}|cpu of a frame over the bytes pending"), &points);
        }
    }
    report.finish();
    // The budget: a streamed frame of an open block of up to 64 KiB takes less than one
    // frame time of CPU. The first frame, which renders the whole head, is not streamed.
    if let Some((max, rung)) = slowest {
        assert!(max < Duration::from_millis(16), "{rung}: a streamed frame took {max:?} of CPU");
    }
    // The budget of a fenced block in a list item (slice 9): each line commits when it
    // ends, so a frame writes the line in progress only, and its bytes do not grow
    // with the block: at most twice those of the smallest rung. No frame takes more
    // than 2 ms of CPU.
    let smallest: Vec<(String, usize)> = fences
        .iter()
        .filter(|(_, bytes, ..)| *bytes == 256)
        .map(|(rung, _, max, _)| (rung.replace("|256 bytes pending", ""), *max))
        .collect();
    for (rung, _, max, cpu) in &fences {
        let Some((_, first)) = smallest.iter().find(|(case, _)| rung.starts_with(case.as_str()))
        else {
            continue;
        };
        assert!(max <= &(2 * first), "{rung}: a frame wrote {max} bytes, the smallest {first}");
        assert!(*cpu <= Duration::from_millis(2), "{rung}: a frame took {cpu:?} of CPU");
    }
}
