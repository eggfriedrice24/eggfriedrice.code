use bytes::Bytes;
use efr_holder::Size;
use efr_protocol::{Cell, Cursor, RowCells, ScreenSnapshot};
use efr_screen::ScreenError;
use pretty_assertions::assert_eq;

use super::{
    FULL_SCREEN_NOTE, MAX_ROWS, Replayer, Scan, TRUSTED_SCROLLBACK, middle_line_end, rows_needed,
    screen_text,
};
use crate::ScreenFactory;
use crate::capture::{Capture, clean};
use crate::testing::{CountingScreens, DyingScreens, NoScreens, Vt100Screens};

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
    const { assert!(TRUSTED_SCROLLBACK <= efr_screen_vt100::DEFAULT_SCROLLBACK_ROWS) };
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
