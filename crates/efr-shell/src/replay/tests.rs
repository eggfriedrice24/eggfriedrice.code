use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use efr_holder::Size;
use efr_protocol::{Cell, Cursor, RowCells, ScreenSnapshot};
use efr_screen::{Screen, ScreenActor, ScreenError, ScreenEvents, ScreenHandle, ScreenSink};
use efr_test_support::bench::Report;
use pretty_assertions::assert_eq;

use super::{
    FULL_SCREEN_NOTE, MAX_ROWS, Replayer, Scan, TRUSTED_SCROLLBACK, middle_line_end, rows_needed,
    screen_text,
};
use crate::capture::{Capture, clean};
use crate::testing::{CountingScreens, DyingScreens, NoScreens, Vt100Screens};
use crate::{RunRequest, ScreenFactory};

const SIZE: Size = Size { cols: 40, rows: 10 };

/// A two-line progress display drawn three times, moving up two rows to redraw.
const DISPLAY: &[u8] = b"layer a: 10%\r\nlayer b: 0%\r\n\
    \x1b[2Alayer a: 60%\r\nlayer b: 30%\r\n\
    \x1b[2A\x1b[2Klayer a: done\r\n\x1b[2Klayer b: done\r\n";

fn replayer(screens: &dyn ScreenFactory, size: Size) -> Replayer<'_> {
    Replayer::new(screens, "replay-test".to_owned(), size)
}

async fn text_at(size: Size, bytes: &[u8]) -> String {
    replayer(&Vt100Screens, size).text(&Bytes::copy_from_slice(bytes)).await
}

async fn text(bytes: &[u8]) -> String {
    text_at(SIZE, bytes).await
}

/// `bytes` behind a cursor-home, which changes nothing at the start of a screen but
/// sends them to one.
fn on_screen(bytes: &[u8]) -> Vec<u8> {
    let mut forced = b"\x1b[H".to_vec();
    forced.extend_from_slice(bytes);
    forced
}

#[tokio::test]
async fn a_multi_line_progress_display_leaves_only_its_final_lines() {
    assert_eq!(text(DISPLAY).await, "layer a: done\nlayer b: done");
    // The cleaner alone would give every frame.
    assert!(clean(DISPLAY).contains("layer a: 60%"));
}

#[tokio::test]
async fn a_carriage_return_progress_bar_leaves_its_final_line() {
    let bar = b" 10% [#    ]\r 50% [###  ]\r100% [#####]\r\ndone\r\n";
    assert_eq!(text(&on_screen(bar)).await, "100% [#####]\ndone");
    // Without cursor movement the cleaner reads it, to the same lines.
    assert_eq!(text(bar).await, "100% [#####]\ndone\n");
}

#[tokio::test]
async fn colours_are_dropped() {
    let coloured =
        on_screen(b"\x1b[1;31merror\x1b[0m: \x1b[38;5;208mdisk\x1b[48;2;1;2;3m full\x1b[0m\r\n");
    assert_eq!(text(&coloured).await, "error: disk full");
}

#[tokio::test]
async fn wide_characters_keep_their_text() {
    let wide = on_screen("日本語のテキスト ok\r\n😀 emoji\r\n".as_bytes());
    assert_eq!(text(&wide).await, "日本語のテキスト ok\n😀 emoji");
    // One that wraps to the next row joins it again.
    let narrow = Size { cols: 5, rows: 4 };
    assert_eq!(text_at(narrow, &on_screen("abc日本\r\n".as_bytes())).await, "abc日本");
}

#[tokio::test]
async fn a_long_line_comes_back_whole_with_the_blank_at_its_wrap() {
    let narrow = Size { cols: 10, rows: 4 };
    assert_eq!(text_at(narrow, &on_screen(b"123456789 abc\r\n")).await, "123456789 abc");
    assert_eq!(
        text_at(narrow, &on_screen(b"one two three four five\r\n")).await,
        "one two three four five"
    );
}

#[tokio::test]
async fn trailing_blanks_go_and_leading_blank_lines_stay() {
    assert_eq!(text(&on_screen(b"a   \r\n\r\n\r\n")).await, "a");
    assert_eq!(text(&on_screen(b"\r\n\r\nb\r\n")).await, "\n\nb");
    assert_eq!(text(&on_screen(b"")).await, "");
}

#[tokio::test]
async fn output_over_the_limit_keeps_its_head_the_marker_and_its_tail() {
    let head = b"a 1\r\n\x1b[Aa 2\r\n";
    let tail = b"b 1\r\n\x1b[Ab 2\r\n";
    let mut capture = Capture::new(head.len() + tail.len());
    capture.push(head);
    capture.push(&[b'x'; 100]);
    capture.push(tail);
    let captured = replayer(&Vt100Screens, SIZE).render(&capture.finish()).await;
    assert_eq!(captured.text, "a 2\n[... 100 bytes omitted ...]\nb 2");
    assert!(captured.truncated);
    assert_eq!(captured.bytes, 126);
}

#[tokio::test]
async fn output_within_the_limit_is_one_replay() {
    let mut capture = Capture::new(1024);
    capture.push(DISPLAY);
    let captured = replayer(&Vt100Screens, SIZE).render(&capture.finish()).await;
    assert_eq!(captured.text, "layer a: done\nlayer b: done");
    assert!(!captured.truncated);
    assert_eq!(captured.bytes, DISPLAY.len() as u64);
}

#[tokio::test]
async fn the_cleaner_reads_the_output_when_no_screen_starts() {
    let text = replayer(&NoScreens, SIZE).text(&Bytes::from_static(DISPLAY)).await;
    assert_eq!(text, clean(DISPLAY));
}

#[tokio::test]
async fn the_cleaner_reads_the_output_when_the_screen_dies() {
    let text = replayer(&DyingScreens, SIZE).text(&Bytes::from_static(DISPLAY)).await;
    assert_eq!(text, clean(DISPLAY));
}

#[tokio::test]
async fn plain_output_starts_no_screen_and_keeps_its_tabs() {
    let screens = CountingScreens::default();
    let plain = b"a\tb\r\n\x1b[31mred\x1b[0m\x1b[K\r\n50%\r100%\r\n";
    let text = replayer(&screens, SIZE).text(&Bytes::from_static(plain)).await;
    assert_eq!(text, "a\tb\nred\n100%\n");
    assert!(screens.captures().is_empty());
}

#[tokio::test]
async fn the_capture_screen_stops_when_the_replay_ends() {
    let screens = CountingScreens::default();
    replayer(&screens, SIZE).text(&Bytes::from_static(DISPLAY)).await;
    let captures = screens.captures();
    assert_eq!(captures.len(), 1);
    let stopped = captures[0].1.snapshot(0).await;
    assert!(matches!(stopped, Err(ScreenError::Closed { .. })), "{stopped:?}");
}

#[tokio::test]
async fn the_capture_screen_has_the_shell_width_and_room_for_the_output() {
    let screens = CountingScreens::default();
    replayer(&screens, SIZE).text(&Bytes::from_static(DISPLAY)).await;
    let mut tall = b"\x1b[2K".to_vec();
    for line in 0..30 {
        tall.extend_from_slice(format!("line {line}\r\n").as_bytes());
    }
    let needed = u16::try_from(rows_needed(&tall, SIZE.cols)).unwrap();
    replayer(&screens, SIZE).text(&Bytes::from(tall)).await;
    let sizes: Vec<Size> = screens.captures().into_iter().map(|(size, _)| size).collect();
    // A short output gets the shell's own height, a taller one room for every line.
    assert_eq!(sizes, [SIZE, Size { cols: 40, rows: needed }]);
    assert!(needed > 30);
}

async fn tail(screens: &dyn ScreenFactory, bytes: &[u8]) -> String {
    replayer(screens, SIZE).tail(&Bytes::copy_from_slice(bytes)).await
}

#[tokio::test]
async fn the_tail_of_a_redrawn_display_is_its_current_frame() {
    assert_eq!(tail(&Vt100Screens, DISPLAY).await, "layer a: done\nlayer b: done");
    // Mid-way, the frame drawn last ends the tail, and no older frame shows.
    let half = &DISPLAY[..DISPLAY.len() - b"\x1b[2Klayer b: done\r\n".len()];
    assert_eq!(tail(&Vt100Screens, half).await, "layer a: done\nlayer b: 30%");
}

#[tokio::test]
async fn the_tail_of_a_carriage_return_bar_is_its_current_line() {
    let bar = b" 10% [#    ]\r 50% [###  ]";
    assert_eq!(tail(&Vt100Screens, bar).await, " 50% [###  ]");
    assert_eq!(tail(&Vt100Screens, &on_screen(bar)).await, " 50% [###  ]");
}

#[tokio::test]
async fn the_tail_reads_one_screen_of_the_shells_size_without_scrollback() {
    let screens = CountingScreens::default();
    let mut tall = b"\x1b[2K".to_vec();
    for line in 0..30 {
        tall.extend_from_slice(format!("line {line}\r\n").as_bytes());
    }
    let text = tail(&screens, &tall).await;
    // Ten rows: the cursor's empty row after nine lines.
    let shown: Vec<String> = (21..30).map(|line| format!("line {line}")).collect();
    assert_eq!(text, shown.join("\n"));
    let captures = screens.captures();
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].0, SIZE);
    let stopped = captures[0].1.snapshot(0).await;
    assert!(matches!(stopped, Err(ScreenError::Closed { .. })), "{stopped:?}");
}

#[tokio::test]
async fn a_tail_without_cursor_movement_starts_no_screen() {
    let screens = CountingScreens::default();
    assert_eq!(tail(&screens, b"a\tb\r\n50%\r100%").await, "a\tb\n100%");
    assert!(screens.captures().is_empty());
}

#[tokio::test]
async fn the_cleaner_reads_the_tail_when_no_screen_starts() {
    assert_eq!(tail(&NoScreens, DISPLAY).await, clean(DISPLAY));
    assert_eq!(tail(&DyingScreens, DISPLAY).await, clean(DISPLAY));
}

#[tokio::test]
async fn the_tail_of_a_full_screen_program_shows_the_main_screen_and_a_note() {
    let program = b"before\r\n\x1b[?1049h\x1b[H\x1b[2Jfull screen ui";
    assert_eq!(tail(&Vt100Screens, program).await, format!("before\n{FULL_SCREEN_NOTE}"));
}

#[tokio::test]
async fn a_full_screen_program_leaves_the_main_screen_and_a_note() {
    let program = b"before\r\n\x1b[?1049h\x1b[H\x1b[2Jfull screen ui\x1b[?1049lafter\r\n";
    assert_eq!(text(program).await, format!("before\nafter\n{FULL_SCREEN_NOTE}"));
}

#[tokio::test]
async fn a_full_screen_program_cut_off_shows_the_main_screen_and_a_note() {
    let expected = format!("before\n{FULL_SCREEN_NOTE}");
    assert_eq!(text(b"before\r\n\x1b[?1049h\x1b[Hui").await, expected);
    // Even in the middle of an escape sequence.
    assert_eq!(text(b"before\r\n\x1b[?1049h\x1b[Hui\x1b[3").await, expected);
}

#[tokio::test]
async fn a_scroll_region_stays_on_the_cleaner() {
    let region = b"\x1b[1;5rone\r\ntwo\r\n\x1b[r";
    assert_eq!(text(region).await, clean(region));
}

#[tokio::test]
async fn a_long_output_is_split_and_keeps_every_line() {
    let screens = CountingScreens::default();
    let mut bytes = b"\x1b[2K".to_vec();
    let mut expected = Vec::new();
    for line in 0..2500 {
        let text = if line % 7 == 0 { String::new() } else { format!("line {line}") };
        bytes.extend_from_slice(format!("{text}\r\n").as_bytes());
        expected.push(text);
    }
    let text = replayer(&screens, SIZE).text(&Bytes::from(bytes)).await;
    assert_eq!(text, expected.join("\n").trim_end_matches('\n'));
    assert!(screens.captures().len() > 1, "the first replay overflowed its scrollback");
}

#[test]
fn the_trusted_scrollback_is_no_more_than_vt100_keeps() {
    const { assert!(TRUSTED_SCROLLBACK <= efr_screen::vt100::DEFAULT_SCROLLBACK_ROWS) };
}

#[test]
fn text_colours_and_carriage_returns_need_no_screen() {
    for plain in [
        &b"plain text\r\n"[..],
        b"\x1b[1;31mred\x1b[0m",
        b"50%\r100%\x1b[K",
        b"50%\r100%\x1b[0K",
        b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        b"\x1b[?25l\x1b[?2004h\x1b[6n\x1b[>1u\x1b[2 q",
        b"\x1b(Bcharset",
    ] {
        assert_eq!(Scan::of(plain), Scan::default(), "{plain:?}");
    }
}

#[test]
fn cursor_movement_and_erasing_need_a_screen() {
    for moving in [
        &b"\x1b[A"[..],
        b"\x1b[3B",
        b"\x1b[10;1H",
        b"\x1b[2K",
        b"\x1b[1K",
        b"\x1b[J",
        b"\x1b[5G",
        b"\x1b7x\x1b8",
        b"\x1bM",
        b"\x1b[s\x1b[u",
        b"\x1b[4h",
        b"\x1b[?7l",
    ] {
        assert!(Scan::of(moving).needs_screen, "{moving:?}");
    }
}

#[test]
fn the_scan_follows_the_alternate_screen_and_scroll_regions() {
    let left = Scan::of(b"\x1b[?1049hui\x1b[?1049l");
    assert!(left.full_screen && !left.alternate_at_end);
    let up = Scan::of(b"\x1b[?1;1049hui");
    assert!(up.full_screen && up.alternate_at_end);
    assert!(Scan::of(b"\x1b[2;20r").scroll_region);
    assert!(!Scan::of(b"\x1b[r").scroll_region);
}

#[test]
fn the_row_bound_counts_line_feeds_and_wraps() {
    assert_eq!(rows_needed(b"ab\r\ncd\r\n", 80), 3);
    assert_eq!(rows_needed(&[b'x'; 100], 40), 3);
    assert_eq!(rows_needed(b"\t\t\t\t\t", 40), 2);
    assert_eq!(rows_needed(b"\x1b[31mred\x1b[0m\x1bD", 80), 2);
}

#[test]
fn a_split_falls_after_the_line_feed_nearest_the_middle() {
    assert_eq!(middle_line_end(b"aa\nbb\ncc"), Some(3));
    assert_eq!(middle_line_end(b"aaaa\nb\nc"), Some(5));
    assert_eq!(middle_line_end(b"abc\n"), None);
    assert_eq!(middle_line_end(b""), None);
}

fn row(text: &str) -> RowCells {
    let cells = text.chars().map(|c| Cell { text: c.to_string(), ..Cell::default() }).collect();
    RowCells { cells, wrapped: false }
}

fn snapshot(rows: Vec<RowCells>, cursor_row: u16) -> ScreenSnapshot {
    ScreenSnapshot {
        size: Size { cols: 4, rows: 4 },
        cursor: Cursor { row: cursor_row, col: 0, hidden: false },
        rows,
        ..ScreenSnapshot::default()
    }
}

#[test]
fn a_piece_that_more_output_follows_keeps_its_blank_lines_above_the_cursor() {
    let screen = snapshot(vec![row("a"), row(""), row(""), row("")], 2);
    assert_eq!(screen_text(&screen, false), "a\n\n");
    assert_eq!(screen_text(&screen, true), "a");
    let blank = snapshot(vec![row(""), row(""), row(""), row("")], 1);
    assert_eq!(screen_text(&blank, false), "\n");
}

#[test]
fn a_wide_character_at_the_end_of_a_wrapped_row_fills_it() {
    let mut first = row("ab");
    first.cells.push(Cell { text: "日".to_owned(), wide: true, ..Cell::default() });
    first.wrapped = true;
    let screen = snapshot(vec![first, row("c"), row(""), row("")], 2);
    assert_eq!(screen_text(&screen, true), "ab日c");
}

#[test]
fn the_capture_screen_is_never_taller_than_its_ceiling() {
    let lines = b"x\r\n".repeat(5000);
    assert_eq!(super::rows_for(&lines, SIZE), MAX_ROWS);
    let big = Size { cols: 300, rows: 400 };
    assert_eq!(super::rows_for(&lines, big), 400);
    assert_eq!(super::rows_for(b"x", SIZE), SIZE.rows);
}

/// A screen that counts the rows that its snapshots build: the history rows (the
/// scrollback) and the visible rows.
struct CountedScreen<S> {
    inner: S,
    counts: Arc<ReplayCounts>,
}

impl<S: Screen> Screen for CountedScreen<S> {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        self.inner.feed(bytes, sink);
    }

    fn resize(&mut self, cols: u16, rows: u16, sink: &mut dyn ScreenSink) {
        self.inner.resize(cols, rows, sink);
    }

    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot {
        let snapshot = self.inner.snapshot(scrollback_rows);
        self.counts.history.fetch_add(snapshot.scrollback.len() as u64, Ordering::SeqCst);
        self.counts.visible.fetch_add(snapshot.rows.len() as u64, Ordering::SeqCst);
        snapshot
    }

    fn row(&self, index: usize) -> RowCells {
        self.inner.row(index)
    }

    fn cursor(&self) -> Cursor {
        self.inner.cursor()
    }

    fn title(&self) -> Option<&str> {
        self.inner.title()
    }

    fn pwd(&self) -> Option<&str> {
        self.inner.pwd()
    }
}

/// What the capture screens of a replay did.
#[derive(Debug, Default)]
struct ReplayCounts {
    spawns: AtomicU64,
    history: AtomicU64,
    visible: AtomicU64,
}

impl ReplayCounts {
    /// The spawns, the history rows and the visible rows so far; they start at 0 again.
    fn take(&self) -> (u64, u64, u64) {
        (
            self.spawns.swap(0, Ordering::SeqCst),
            self.history.swap(0, Ordering::SeqCst),
            self.visible.swap(0, Ordering::SeqCst),
        )
    }
}

/// vt100 screens that count their spawns and whose snapshots count their rows. They keep
/// `scrollback` rows of history, or vt100's default when it is `None`.
#[derive(Debug, Default)]
struct CountedVt100 {
    counts: Arc<ReplayCounts>,
    scrollback: Option<usize>,
}

impl ScreenFactory for CountedVt100 {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        self.counts.spawns.fetch_add(1, Ordering::SeqCst);
        let counts = Arc::clone(&self.counts);
        let rows = self.scrollback.unwrap_or(efr_screen::vt100::DEFAULT_SCROLLBACK_ROWS);
        let screen = move || CountedScreen {
            inner: efr_screen::vt100::Vt100Screen::with_scrollback(size, rows),
            counts,
        };
        ScreenActor::spawn(name, screen, size)
    }
}

/// What one snapshot of a [`CheckedScreen`] asked for and what the screen held.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asked {
    /// The scrollback rows that the replay asked for.
    rows: usize,
    /// The rows of scrollback that the screen held.
    held: usize,
    /// The snapshot is the newest `rows` rows of the one with all the scrollback.
    newest: bool,
}

/// A vt100 screen with `scrollback` rows of history, whose snapshots also take one with
/// all of it, as the replay asked for before it asked for [`TRUSTED_SCROLLBACK`] rows.
struct CheckedScreen {
    inner: efr_screen::vt100::Vt100Screen,
    asked: Arc<std::sync::Mutex<Vec<Asked>>>,
}

impl Screen for CheckedScreen {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        self.inner.feed(bytes, sink);
    }

    fn resize(&mut self, cols: u16, rows: u16, sink: &mut dyn ScreenSink) {
        self.inner.resize(cols, rows, sink);
    }

    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot {
        let all = self.inner.snapshot(usize::MAX);
        let snapshot = self.inner.snapshot(scrollback_rows);
        let newest = all.scrollback.len().saturating_sub(scrollback_rows);
        let asked = Asked {
            rows: scrollback_rows,
            held: all.scrollback.len(),
            newest: snapshot.rows == all.rows && snapshot.scrollback == all.scrollback[newest..],
        };
        self.asked.lock().unwrap().push(asked);
        snapshot
    }

    fn row(&self, index: usize) -> RowCells {
        self.inner.row(index)
    }

    fn cursor(&self) -> Cursor {
        self.inner.cursor()
    }

    fn title(&self) -> Option<&str> {
        self.inner.title()
    }

    fn pwd(&self) -> Option<&str> {
        self.inner.pwd()
    }
}

/// Capture screens over [`CheckedScreen`] with `scrollback` rows of history.
#[derive(Debug, Default)]
struct CheckedScreens {
    scrollback: usize,
    asked: Arc<std::sync::Mutex<Vec<Asked>>>,
}

impl ScreenFactory for CheckedScreens {
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError> {
        let (asked, scrollback) = (Arc::clone(&self.asked), self.scrollback);
        let screen = move || CheckedScreen {
            inner: efr_screen::vt100::Vt100Screen::with_scrollback(size, scrollback),
            asked,
        };
        ScreenActor::spawn(name, screen, size)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replay_reads_only_the_trusted_scrollback_and_its_text_stays_the_same() {
    let size = Size { cols: 160, rows: 48 };
    let output = Bytes::from(status_line_output(256 * 1024));
    let steps = output.windows(12).filter(|part| part == b"   Compiling").count();
    // vt100 keeps 1,000 rows of scrollback; ghostty keeps 5,000, as this one does.
    let mut texts = Vec::new();
    for scrollback in [efr_screen::vt100::DEFAULT_SCROLLBACK_ROWS, 5000] {
        let screens = CheckedScreens { scrollback, ..CheckedScreens::default() };
        let text = replayer(&screens, size).text(&output).await;
        let asked = screens.asked.lock().unwrap().clone();

        assert!(asked.len() > 2, "{scrollback}: the output was split: {asked:?}");
        for asked in &asked {
            assert_eq!(asked.rows, TRUSTED_SCROLLBACK, "{scrollback}: {asked:?}");
            // The rows are the newest of all the scrollback, and the replay splits the
            // same pieces as with all of it.
            assert!(asked.newest, "{scrollback}: {asked:?}");
        }
        let compiled = text.lines().filter(|line| line.starts_with("   Compiling")).count();
        assert_eq!(compiled, steps, "{scrollback}: every log line is kept");
        texts.push((asked.len(), text));
    }
    assert_eq!(texts[0], texts[1], "both backends give the same splits and the same text");
}

/// Build output under a status line that moves up to redraw (`ESC [1A`): each step
/// erases the status line, prints one log line and draws the status line again, so the
/// output both moves the cursor and grows the scrollback by one row per step. At least
/// `size` bytes long.
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

/// The replay of cursor-moving output on vt100: the capture screens started, the rows
/// their snapshots built and the time of `Replayer::render`, on 512 KiB and 5 MiB kept
/// whole, and on 5 MiB kept as a run keeps it (1 MiB: the head and the tail). The
/// ghostty backend cannot run here, because efr-shell never names a backend:
/// `bench_perf_replay_screen` of efr-screen-ghostty runs the same loop on both. A vt100
/// that keeps 5,000 rows of scrollback, as ghostty does, stands in for it.
///
/// Budget: no snapshot builds more than [`TRUSTED_SCROLLBACK`] history rows (plan slice
/// W1-J); before, a screen that kept 5,000 rows gave them all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_replay() {
    let mut report = Report::new("bench_perf_replay");
    report.count("skipped: ghostty; see efr-screen-ghostty bench_perf_replay_screen", 0);
    let size = Size { cols: 160, rows: 48 };
    let vt100 = CountedVt100::default();
    let deep = CountedVt100 { scrollback: Some(5000), ..CountedVt100::default() };
    let mut points = Vec::new();
    let mut over = Vec::new();
    let cases = [
        ("vt100 512 KiB", &vt100, 512 * 1024, 512 * 1024),
        ("vt100 5 MiB", &vt100, 5 * 1024 * 1024, 5 * 1024 * 1024),
        ("vt100 5 MiB kept 1 MiB", &vt100, 5 * 1024 * 1024, RunRequest::DEFAULT_OUTPUT_LIMIT),
        ("vt100 5000 rows 5 MiB", &deep, 5 * 1024 * 1024, 5 * 1024 * 1024),
    ];
    for (label, screens, bytes, limit) in cases {
        let replayer = Replayer::new(screens, "replay-bench".to_owned(), size);
        let output = status_line_output(bytes);
        let mut capture = Capture::new(limit);
        capture.push(&output);
        let kept = capture.finish();
        let mut text = String::new();
        let mut samples = Vec::new();
        let mut counts = (0, 0, 0);
        for _ in 0..3 {
            let start = Instant::now();
            text = replayer.render(&kept).await.text;
            samples.push(start.elapsed());
            counts = screens.counts.take();
        }
        assert!(text.contains("Compiling crate-"), "{label}: the replay kept the log lines");
        let stats = report.time(label, &samples);
        let (spawns, history, visible) = counts;
        report.count(&format!("{label} screen spawns"), spawns);
        report.count(&format!("{label} history rows"), history);
        report.count(&format!("{label} visible rows"), visible);
        report.count(&format!("{label} lines out"), text.lines().count() as u64);
        if history > spawns * TRUSTED_SCROLLBACK as u64 {
            over.push(format!("{label}: {history} history rows from {spawns} screens"));
        }
        if limit >= bytes && screens.scrollback.is_none() {
            points.push((bytes as u64, stats.p50));
        }
        if stats.p50 > Duration::from_secs(10) {
            report.count("stopped_at_n", bytes as u64);
            break;
        }
    }
    report.slope("vt100 slope", &points);
    report.finish();
    assert!(over.is_empty(), "{over:?}");
}
