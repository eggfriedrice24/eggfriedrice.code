use efr_screen::row_text;
use pretty_assertions::assert_eq;

use super::*;
use crate::test_sink::{Call, TestSink};

fn size(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

/// A screen of `cols` by `rows` with `scrollback_rows` of scrollback after `bytes`.
fn fed(cols: u16, rows: u16, scrollback_rows: usize, bytes: &[u8]) -> Vt100Screen {
    let mut screen = Vt100Screen::with_scrollback(size(cols, rows), scrollback_rows);
    screen.feed(bytes, &mut TestSink::default());
    screen
}

fn texts(rows: &[RowCells]) -> Vec<String> {
    rows.iter().map(row_text).collect()
}

/// Lines `1` to `count`, one per row, the last without a line feed.
fn numbered_lines(count: usize) -> Vec<u8> {
    (1..=count).map(|line| line.to_string()).collect::<Vec<_>>().join("\r\n").into_bytes()
}

#[test]
fn a_new_screen_is_blank_at_its_size() {
    let mut screen = Vt100Screen::new(size(10, 3));
    let snapshot = screen.snapshot(100);
    assert_eq!(snapshot.size, size(10, 3));
    assert_eq!(snapshot.cursor, Cursor::default());
    assert_eq!(texts(&snapshot.rows), ["", "", ""]);
    assert_eq!(snapshot.scrollback, []);
    assert_eq!(snapshot.title, None);
    assert!(!snapshot.alternate_screen);
    assert_eq!(screen.pwd(), None);
}

#[test]
fn the_default_scrollback_is_kept() {
    let lines = DEFAULT_SCROLLBACK_ROWS + 10;
    let mut screen = Vt100Screen::new(size(8, 2));
    screen.feed(&numbered_lines(lines), &mut TestSink::default());
    let scrollback = screen.snapshot(usize::MAX).scrollback;
    assert_eq!(scrollback.len(), DEFAULT_SCROLLBACK_ROWS);
    assert_eq!(row_text(&scrollback[0]), (lines - 1 - DEFAULT_SCROLLBACK_ROWS).to_string());
}

#[test]
fn a_dimension_of_zero_becomes_one() {
    let mut screen = Vt100Screen::new(size(0, 0));
    let snapshot = screen.snapshot(0);
    assert_eq!(snapshot.size, size(1, 1));
    assert_eq!(snapshot.rows.len(), 1);
    screen.resize(0, 4, &mut TestSink::default());
    assert_eq!(screen.snapshot(0).size, size(1, 4));
    screen.resize(6, 0, &mut TestSink::default());
    assert_eq!(screen.snapshot(0).size, size(6, 1));
}

#[test]
fn a_one_row_screen_shows_the_row_the_cursor_is_on() {
    let mut screen = fed(10, 1, 10, b"one\r\ntwo\r\nthree");
    let snapshot = screen.snapshot(10);
    assert_eq!(snapshot.size, size(10, 1));
    assert_eq!(texts(&snapshot.rows), ["three"]);
    assert_eq!(texts(&snapshot.scrollback), ["one", "two"]);
    assert_eq!(snapshot.cursor, Cursor { row: 0, col: 5, hidden: false });
    assert_eq!(row_text(&screen.row(0)), "three");
    assert_eq!(screen.row(1), RowCells::default());
    assert_eq!(texts(&screen.snapshot(1).scrollback), ["two"]);
    assert_eq!(screen.snapshot(0).scrollback, []);
}

#[test]
fn a_line_wraps_on_a_one_row_screen() {
    let mut screen = fed(4, 1, 10, b"abcdefghij");
    let snapshot = screen.snapshot(10);
    assert_eq!(texts(&snapshot.rows), ["ij"]);
    assert_eq!(texts(&snapshot.scrollback), ["abcd", "efgh"]);
    assert!(snapshot.scrollback.iter().all(|row| row.wrapped));
}

#[test]
fn moving_up_on_a_one_row_screen_shows_the_row_above() {
    // The documented approximation: a real single row ignores the move.
    let mut screen = fed(10, 1, 10, b"first\r\nsecond\x1b[A");
    let snapshot = screen.snapshot(10);
    assert_eq!(texts(&snapshot.rows), ["first"]);
    assert_eq!(snapshot.cursor.row, 0);
    assert_eq!(snapshot.scrollback, []);
}

#[test]
fn a_one_row_screen_grows_into_both_of_its_rows() {
    let mut screen = fed(10, 1, 10, b"one\r\ntwo");
    screen.resize(10, 3, &mut TestSink::default());
    let snapshot = screen.snapshot(10);
    assert_eq!(texts(&snapshot.rows), ["one", "two", ""]);
    assert_eq!(snapshot.cursor, Cursor { row: 1, col: 3, hidden: false });
    assert_eq!(snapshot.scrollback, []);
}

#[test]
fn a_one_column_screen_wraps_every_character() {
    let mut screen = fed(1, 3, 10, b"abcd");
    let snapshot = screen.snapshot(10);
    assert_eq!(texts(&snapshot.rows), ["b", "c", "d"]);
    assert_eq!(texts(&snapshot.scrollback), ["a"]);
    assert_eq!(snapshot.cursor, Cursor { row: 2, col: 0, hidden: false });
}

#[test]
fn feed_reports_bells_and_titles_in_the_order_they_happened() {
    let mut screen = Vt100Screen::new(size(10, 2));
    let mut sink = TestSink::default();
    screen.feed(b"a\x07\x1b]2;make\x07b\x07\x1b]0;done\x1b\\", &mut sink);
    assert_eq!(
        sink.take(),
        [Call::Bell, Call::Title("make".to_owned()), Call::Bell, Call::Title("done".to_owned())]
    );
    screen.feed(b"plain", &mut sink);
    assert_eq!(sink.take(), []);
}

#[test]
fn a_title_split_across_feeds_is_reported_once_complete() {
    let mut screen = Vt100Screen::new(size(10, 2));
    let mut sink = TestSink::default();
    screen.feed(b"\x1b]2;ma", &mut sink);
    assert_eq!(sink.take(), []);
    screen.feed(b"ke\x07", &mut sink);
    assert_eq!(sink.take(), [Call::Title("make".to_owned())]);
}

#[test]
fn terminal_queries_get_no_answer() {
    let mut screen = Vt100Screen::new(size(10, 2));
    let mut sink = TestSink::default();
    // DA1, DA2, DSR, CPR, DECRQM, XTVERSION, OSC 10 and OSC 11.
    let queries = b"\x1b[c\x1b[>c\x1b[5n\x1b[6n\x1b[?2004$p\x1b[>q\x1b]10;?\x07\x1b]11;?\x1b\\";
    screen.feed(queries, &mut sink);
    assert_eq!(sink.take(), []);
    assert_eq!(texts(&screen.snapshot(0).rows), ["", ""]);
}

#[test]
fn resize_changes_the_grid_and_tells_the_sink_nothing() {
    let mut screen = fed(5, 2, 0, b"hey");
    let mut sink = TestSink::default();
    screen.resize(10, 3, &mut sink);
    assert_eq!(sink.take(), []);
    let snapshot = screen.snapshot(0);
    assert_eq!(snapshot.size, size(10, 3));
    assert_eq!(texts(&snapshot.rows), ["hey", "", ""]);
    assert_eq!(snapshot.cursor, Cursor { row: 0, col: 3, hidden: false });
}

#[test]
fn shrinking_drops_the_bottom_rows_and_keeps_the_cursor_inside() {
    // vt100 does not push rows into the scrollback when the grid shrinks, unlike
    // libghostty-vt; the README lists it among the differences.
    let mut screen = fed(5, 3, 10, b"1\r\n2\r\n3");
    screen.resize(5, 2, &mut TestSink::default());
    let snapshot = screen.snapshot(10);
    assert_eq!(texts(&snapshot.rows), ["1", "2"]);
    assert_eq!(snapshot.scrollback, []);
    assert_eq!(snapshot.cursor.row, 1);
}

#[test]
fn the_cursor_stays_on_the_last_column_while_a_wrap_is_pending() {
    let screen = fed(5, 2, 0, b"abcde");
    assert_eq!(screen.parser.screen().cursor_position(), (0, 5));
    assert_eq!(screen.cursor(), Cursor { row: 0, col: 4, hidden: false });
}

#[test]
fn the_cursor_reports_dectcem() {
    let mut screen = fed(5, 2, 0, b"\x1b[?25l");
    assert!(screen.cursor().hidden);
    assert!(screen.snapshot(0).cursor.hidden);
    screen.feed(b"\x1b[?25h", &mut TestSink::default());
    assert!(!screen.cursor().hidden);
}

#[test]
fn scrollback_is_oldest_first_and_keeps_the_newest_rows_asked_for() {
    let mut screen = fed(5, 3, 100, &numbered_lines(9));
    let snapshot = screen.snapshot(2);
    assert_eq!(texts(&snapshot.rows), ["7", "8", "9"]);
    assert_eq!(texts(&snapshot.scrollback), ["5", "6"]);
    assert_eq!(texts(&screen.snapshot(100).scrollback), ["1", "2", "3", "4", "5", "6"]);
    assert_eq!(screen.snapshot(0).scrollback, []);
}

#[test]
fn scrollback_longer_than_the_screen_is_read_a_screen_at_a_time() {
    let mut screen = fed(5, 2, 100, &numbered_lines(13));
    let expected: Vec<String> = (1..=11).map(|line| line.to_string()).collect();
    assert_eq!(texts(&screen.snapshot(100).scrollback), expected);
    assert_eq!(texts(&screen.snapshot(5).scrollback), expected[6..]);
}

#[test]
fn scrollback_keeps_attributes_and_wrapping() {
    let mut screen = fed(4, 2, 10, b"\x1b[1mabcdefghij");
    let scrollback = screen.snapshot(10).scrollback;
    assert_eq!(scrollback.len(), 1);
    assert!(scrollback[0].wrapped);
    assert!(scrollback[0].cells.iter().all(|cell| cell.bold));
    assert_eq!(row_text(&scrollback[0]), "abcd");
}

#[test]
fn a_snapshot_leaves_the_view_at_the_bottom() {
    let mut screen = fed(5, 2, 100, &numbered_lines(6));
    let _ = screen.snapshot(100);
    assert_eq!(screen.parser.screen().scrollback(), 0);
    assert_eq!(row_text(&screen.row(0)), "5");
    screen.feed(b"\r\n7", &mut TestSink::default());
    assert_eq!(texts(&screen.snapshot(1).rows), ["6", "7"]);
    assert_eq!(texts(&screen.snapshot(1).scrollback), ["5"]);
}

#[test]
fn no_scrollback_means_none_is_kept() {
    let mut screen = fed(5, 2, 0, &numbered_lines(6));
    assert_eq!(screen.snapshot(100).scrollback, []);
}

#[test]
fn the_alternate_screen_is_reported_and_shows_no_scrollback() {
    let mut screen = fed(5, 2, 100, &numbered_lines(4));
    screen.feed(b"\x1b[?1049hfull", &mut TestSink::default());
    let snapshot = screen.snapshot(100);
    assert!(snapshot.alternate_screen);
    assert_eq!(texts(&snapshot.rows), ["full", ""]);
    assert_eq!(snapshot.scrollback, []);

    screen.feed(b"\x1b[?1049l", &mut TestSink::default());
    let snapshot = screen.snapshot(100);
    assert!(!snapshot.alternate_screen);
    assert_eq!(texts(&snapshot.rows), ["3", "4"]);
    assert_eq!(texts(&snapshot.scrollback), ["1", "2"]);
}

#[test]
fn row_matches_the_snapshot_rows() {
    let mut screen = fed(6, 3, 0, b"\x1b[31mred\r\nplain");
    let snapshot = screen.snapshot(0);
    for (index, row) in snapshot.rows.iter().enumerate() {
        let mut direct = screen.row(index);
        direct.cells.truncate(row.cells.len());
        assert_eq!(&direct, row, "row {index}");
    }
}

#[test]
fn a_row_outside_the_grid_is_empty() {
    let screen = fed(6, 3, 10, &numbered_lines(5));
    assert_eq!(screen.row(3), RowCells::default());
    assert_eq!(screen.row(usize::from(u16::MAX) + 1), RowCells::default());
    assert_eq!(screen.row(usize::MAX), RowCells::default());
}

#[test]
fn title_and_pwd_come_from_the_stream() {
    let mut screen = fed(10, 2, 0, b"\x1b]2;vim\x07\x1b]7;file://arch/etc\x07");
    assert_eq!(screen.title(), Some("vim"));
    assert_eq!(screen.pwd(), Some("file://arch/etc"));
    assert_eq!(screen.snapshot(0).title.as_deref(), Some("vim"));
}

#[test]
fn osc_133_and_osc_7_print_nothing() {
    let mut screen =
        fed(10, 2, 0, b"\x1b]133;A;cl=line\x07\x1b]7;file://arch/tmp\x07$ \x1b]133;B\x07");
    assert_eq!(texts(&screen.snapshot(0).rows), ["$", ""]);
    assert_eq!(screen.cursor(), Cursor { row: 0, col: 2, hidden: false });
}

#[test]
fn the_factory_builds_a_screen_of_the_size() {
    let mut screen = factory(size(12, 4))();
    assert_eq!(screen.snapshot(0).size, size(12, 4));
}

#[test]
fn a_screen_and_its_factory_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    let factory = factory(size(3, 3));
    assert_send(&factory);
    assert_send(&factory());
}

#[test]
fn debug_shows_the_state_but_not_the_grid() {
    let screen = fed(10, 2, 0, b"secret\x1b]2;vim\x07");
    let debug = format!("{screen:?}");
    assert!(debug.starts_with("Vt100Screen {"), "{debug}");
    assert!(debug.contains("cols: 10, rows: 2"), "{debug}");
    assert!(debug.contains("Some(\"vim\")"), "{debug}");
    assert!(!debug.contains("secret"), "{debug}");
}
