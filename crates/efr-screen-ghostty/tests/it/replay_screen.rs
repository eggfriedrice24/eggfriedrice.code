//! The bench of the capture screens of a replay on both backends. efr-shell replays
//! output that moves the cursor on a new screen per piece and reads it back with all
//! its scrollback; a piece whose scrollback holds 1,000 rows or more is replayed again
//! in two halves (`efr-shell/src/replay.rs`). efr-shell names no backend, so its own
//! bench (`bench_perf_replay`) runs only vt100; this one runs the same loop on vt100
//! and on ghostty, which keeps 5,000 rows of scrollback, and reads each screen in three
//! ways: a snapshot with all its scrollback, a snapshot with 1,000 rows (plan slice
//! W1-J), and `Screen::text` with the limit of 1,000 rows, which efr-shell uses now. It
//! reports the time, the screens, and the history rows that the snapshots built or the
//! bytes of text. A bench, ignored: `just bench-perf` runs it in release.
//!
//! NOTE: the loop copies the split rule of `efr-shell/src/replay.rs` (`replay`,
//! `rows_for`, `middle_line_end`) and leaves out the text of the snapshot rows, so it
//! measures the screens alone. The rows of a capture screen are approximated by the
//! line feeds.

use std::time::{Duration, Instant};

use efr_screen::{Screen, ScreenSink, Seq, ShellMarkScanner, Size};
use efr_test_support::bench::Report;

/// The scrollback rows that every backend keeps at least (`TRUSTED_SCROLLBACK`).
const TRUSTED_SCROLLBACK: usize = 1000;

/// The tallest capture screen (`MAX_ROWS`).
const MAX_ROWS: u16 = 200;

/// The shell's size.
const SHELL: Size = Size { cols: 160, rows: 48 };

/// A sink that drops what a screen produces.
struct Discard;

impl ScreenSink for Discard {
    fn pty_reply(&mut self, _bytes: &[u8]) {}

    fn bell(&mut self) {}

    fn title_changed(&mut self, _title: &str) {}
}

/// Build output under a status line that moves up to redraw (`ESC [1A`), as in
/// efr-shell's bench: one more row of scrollback per step. At least `size` bytes.
fn status_line_output(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size + 256);
    let mut n = 0_u64;
    out.extend_from_slice(b"\r\n");
    while out.len() < size {
        let bar = "=".repeat(usize::try_from(n % 20).unwrap());
        let step = format!(
            "\x1b[1A\r\x1b[2K   Compiling crate-{n} v0.{}.0\r\n\x1b[2K    Building [{bar:<20}] \
             {n}/99999\r\n",
            n % 50
        );
        out.extend_from_slice(step.as_bytes());
        n += 1;
    }
    out
}

/// The index after the line feed nearest the middle of `bytes`, with both halves
/// holding something.
fn middle_line_end(bytes: &[u8]) -> Option<usize> {
    let middle = bytes.len() / 2;
    let before = bytes[..middle].iter().rposition(|&byte| byte == b'\n').map(|at| at + 1);
    let after = bytes[middle..].iter().position(|&byte| byte == b'\n').map(|at| middle + at + 1);
    [before, after]
        .into_iter()
        .flatten()
        .filter(|&at| at > 0 && at < bytes.len())
        .min_by_key(|&at| at.abs_diff(middle))
}

/// What one replay did.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    screens: u64,
    history: u64,
    bytes_fed: u64,
    text_bytes: u64,
}

/// How a replay reads each screen.
#[derive(Debug, Clone, Copy)]
enum Read {
    /// A snapshot with up to this many rows of scrollback, as efr-shell did before
    /// `Screen::text`.
    Snapshot(usize),
    /// `Screen::text` with the trusted limit, as efr-shell does now.
    Text,
}

/// Replays `bytes` as efr-shell does, on screens from `make`, reading each with `read`.
fn replay<S: Screen>(make: &dyn Fn(Size) -> S, bytes: &[u8], read: Read) -> Counts {
    let mut counts = Counts::default();
    let mut pieces: Vec<&[u8]> = vec![bytes];
    while let Some(piece) = pieces.pop() {
        let feeds = piece.iter().filter(|&&byte| byte == b'\n').count() + 1;
        let rows = u16::try_from(feeds).unwrap_or(u16::MAX).clamp(SHELL.rows, MAX_ROWS);
        let mut screen = make(Size { cols: SHELL.cols, rows });
        // As the actor does: one scan for marks, and the marks go with the bytes.
        let marks = ShellMarkScanner::new().scan(piece, Seq::ZERO);
        screen.feed_marked(piece, Seq::ZERO, &marks, &mut Discard);
        counts.screens += 1;
        counts.bytes_fed += piece.len() as u64;
        let full = match read {
            Read::Snapshot(ask) => {
                let snapshot = screen.snapshot(ask);
                counts.history += snapshot.scrollback.len() as u64;
                snapshot.scrollback.len() >= TRUSTED_SCROLLBACK
            }
            Read::Text => match screen.text(TRUSTED_SCROLLBACK) {
                Some(text) => {
                    counts.text_bytes += text.text.len() as u64;
                    false
                }
                None => true,
            },
        };
        if !full {
            continue;
        }
        if let Some(at) = middle_line_end(piece) {
            pieces.push(&piece[at..]);
            pieces.push(&piece[..at]);
        }
    }
    counts
}

/// Runs the cases of one backend.
fn backend<S: Screen>(report: &mut Report, name: &str, make: &dyn Fn(Size) -> S) {
    for (size_name, size) in [("512 KiB", 512 * 1024), ("5 MiB", 5 * 1024 * 1024)] {
        let output = status_line_output(size);
        for (read_name, read) in [
            ("scrollback all", Read::Snapshot(usize::MAX)),
            ("scrollback 1000", Read::Snapshot(TRUSTED_SCROLLBACK)),
            ("text", Read::Text),
        ] {
            let label = format!("{name} {size_name} {read_name}");
            let mut samples = Vec::new();
            let mut counts = Counts::default();
            for _ in 0..3 {
                let start = Instant::now();
                counts = replay(make, &output, read);
                samples.push(start.elapsed());
            }
            let stats = report.time(&label, &samples);
            report.count(&format!("{label} screens"), counts.screens);
            match read {
                Read::Snapshot(_) => report.count(&format!("{label} history rows"), counts.history),
                Read::Text => report.bytes(&format!("{label} text"), counts.text_bytes),
            }
            report.bytes(&format!("{label} bytes fed"), counts.bytes_fed);
            if stats.p50 > Duration::from_secs(10) {
                report.count(&format!("{name} stopped_at_n"), size as u64);
                return;
            }
        }
    }
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_replay_screen() {
    let mut report = Report::new("bench_perf_replay_screen");
    backend(&mut report, "vt100", &|size| efr_screen::vt100::factory(size)());
    backend(&mut report, "ghostty", &|size| efr_screen_ghostty::factory(size)());
    report.finish();
}
