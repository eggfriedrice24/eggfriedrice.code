//! The benches of the view (`just bench-perf`, section 2.3 of the perf plan): the
//! frames of a pending approval with a diff, and a long reply that streams in small
//! drafts. Each one has a budget on a count or a slope.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use efr_test_support::bench::Report;

use super::super::{Ask, Look};
use super::{ALL, approval, at, sent, started_view, text_draft, turn_started};
use crate::format::card::RENDERS;
use crate::terminal::Size;
use crate::testing::{call, now};

const SCREEN: Size = Size { cols: 80, rows: 40 };

/// A size of a ladder that takes longer than this stops the ladder.
const LIMIT: Duration = Duration::from_secs(10);

/// A diff preview of `lines` changed lines, as the daemon sends one for a write.
fn diff_preview(lines: usize) -> String {
    let mut diff =
        String::from("--- a/home/user/.zshrc\n+++ b/home/user/.zshrc\n@@ -1,3 +1,3 @@\n");
    for at in 0..lines {
        let mark = if at % 2 == 0 { '-' } else { '+' };
        let _ = writeln!(diff, "{mark}export PATH=\"$HOME/.local/bin:$PATH\" # line {at}");
    }
    diff
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_card_frames() {
    const TICKS: i64 = 600;
    let mut view = started_view(ALL);
    view.envelope(&sent(11, 0, turn_started()), SCREEN, false);
    let step = view.event(&approval(Some(&diff_preview(30))), SCREEN, true);
    assert_eq!(step.ask, Some(Ask::Approval(call())));
    let first = view.frame(SCREEN, at(0));
    assert!(first.contains("allow"), "the card is not on the screen");

    RENDERS.with(|renders| renders.set(0));
    let mut ticks = Vec::new();
    let mut written = 0;
    for tick in 1..=TICKS {
        let start = Instant::now();
        let frame = view.tick(SCREEN, at(tick * 100));
        ticks.push(start.elapsed());
        written += frame.len();
    }
    let renders = RENDERS.with(std::cell::Cell::get);

    let mut report = Report::new("bench_perf_card_frames");
    report.time("tick with a pending diff approval", &ticks);
    report.time("600 ticks: total", &[ticks.iter().sum()]);
    report.count("Card::render calls over 600 ticks", renders);
    report.bytes("written over 600 ticks", written as u64);
    report.finish();
    // Budget: a tick that changes neither the card's footer nor the width renders no
    // card (it rendered two per tick).
    assert_eq!(renders, 0, "Card::render calls over {TICKS} ticks with no change");
}

/// A reply of `bytes` bytes of prose: lines of about 80 columns, a blank line after
/// every fourth line.
fn reply(bytes: usize) -> String {
    let mut text = String::new();
    let mut line = 0;
    while text.len() < bytes {
        let _ = writeln!(
            text,
            "Line {line} of the reply says what the build did and why the test failed there."
        );
        line += 1;
        if line % 4 == 0 {
            text.push('\n');
        }
    }
    text.truncate(bytes);
    text
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_message_delta() {
    const DELTA: usize = 16;
    // A frame for each 1 KB of drafts, about what the screen sees of a fast model.
    const FRAME_EVERY: usize = 64;
    let mut report = Report::new("bench_perf_message_delta");
    let mut slopes = Vec::new();
    for frames in [false, true] {
        let kind = if frames { "drafts and a frame per 1 KB" } else { "drafts only" };
        let mut points = Vec::new();
        // Without the last frame, which renders all that waits at once.
        let mut draft_points = Vec::new();
        for kb in [10_u64, 100, 1_000] {
            let text = reply(kb as usize * 1024);
            let mut view = started_view(Look::default());
            view.envelope(&sent(11, 0, turn_started()), SCREEN, false);
            let mut written = 0;
            let mut stopped = false;
            let start = Instant::now();
            for (at, offset) in (0..text.len()).step_by(DELTA).enumerate() {
                let delta = &text[offset..(offset + DELTA).min(text.len())];
                view.draft(&text_draft(11, offset, delta), SCREEN);
                if frames && at % FRAME_EVERY == FRAME_EVERY - 1 {
                    written += view.frame(SCREEN, now()).len();
                }
                if start.elapsed() > LIMIT {
                    stopped = true;
                    break;
                }
            }
            let drafts = start.elapsed();
            written += view.frame(SCREEN, now()).len();
            let total = start.elapsed();
            report.time(&format!("{kind} {kb} KB: total"), &[total]);
            report.time(&format!("{kind} {kb} KB: drafts"), &[drafts]);
            if stopped {
                report.count(&format!("{kind} stopped_at_kb"), kb);
                break;
            }
            assert!(written > 0, "the reply never reached the screen");
            points.push((kb, total));
            draft_points.push((kb, drafts));
        }
        report.slope(&format!("{kind} slope over bytes"), &points);
        let slope = report.slope(&format!("{kind} slope of the drafts over bytes"), &draft_points);
        slopes.push((kind, slope));
    }
    report.finish();
    // Budget: a draft costs its delta, not the text before it (the slope was 1.7).
    for (kind, slope) in slopes {
        assert!(slope < 1.3, "{kind}: slope {slope:.2} of the drafts over bytes");
    }
}
