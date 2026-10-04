use efr_screen::{Cursor, RowCells, Size};
use pretty_assertions::assert_eq;

use super::{active_row, capture, cursor, decode, encode, title};
use crate::testing::{terminal, texts};

#[test]
fn capture_holds_every_visible_row_at_full_width() {
    let (mut terminal, _effects) = terminal(6, 3);
    terminal.vt_write(b"ab\r\ncd");
    let snapshot = capture(&terminal, 0);
    assert_eq!(snapshot.size, Size { cols: 6, rows: 3 });
    assert_eq!(texts(&snapshot.rows), vec!["ab", "cd", ""]);
    // The actor trims trailing blanks; the backend hands over whole rows.
    assert!(snapshot.rows.iter().all(|row| row.cells.len() == 6));
}

#[test]
fn scrollback_is_oldest_first_and_keeps_the_newest_rows() {
    let (mut terminal, _effects) = terminal(5, 3);
    terminal.vt_write(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
    assert_eq!(texts(&capture(&terminal, 100).scrollback), vec!["1", "2", "3"]);
    assert_eq!(texts(&capture(&terminal, 2).scrollback), vec!["2", "3"]);
    assert_eq!(capture(&terminal, 0).scrollback, Vec::<RowCells>::new());
    assert_eq!(texts(&capture(&terminal, 0).rows), vec!["4", "5", "6"]);
}

#[test]
fn the_cursor_reports_dectcem() {
    let (mut terminal, _effects) = terminal(5, 2);
    terminal.vt_write(b"\r\nab\x1b[?25l");
    assert_eq!(cursor(&terminal), Cursor { row: 1, col: 2, hidden: true });
    terminal.vt_write(b"\x1b[?25h");
    assert!(!cursor(&terminal).hidden);
}

#[test]
fn a_pending_wrap_keeps_the_cursor_on_the_last_column() {
    let (mut terminal, _effects) = terminal(5, 2);
    terminal.vt_write(b"abcde");
    assert_eq!(cursor(&terminal), Cursor { row: 0, col: 4, hidden: false });
}

#[test]
fn the_alternate_screen_is_reported() {
    let (mut terminal, _effects) = terminal(5, 2);
    assert!(!capture(&terminal, 0).alternate_screen);
    terminal.vt_write(b"\x1b[?1049h");
    assert!(capture(&terminal, 0).alternate_screen);
    terminal.vt_write(b"\x1b[?1049l");
    assert!(!capture(&terminal, 0).alternate_screen);
}

#[test]
fn there_is_no_title_until_a_program_sets_one() {
    let (mut terminal, _effects) = terminal(5, 2);
    assert_eq!(title(&terminal), None);
    assert_eq!(capture(&terminal, 0).title, None);
    terminal.vt_write(b"\x1b]0;vim\x07");
    assert_eq!(capture(&terminal, 0).title.as_deref(), Some("vim"));
}

#[test]
fn rows_outside_the_grid_are_empty() {
    let (mut terminal, _effects) = terminal(5, 2);
    terminal.vt_write(b"ab\r\ncd");
    assert_eq!(texts(&[active_row(&terminal, 1)]), vec!["cd"]);
    assert_eq!(active_row(&terminal, 2), RowCells::default());
    assert_eq!(active_row(&terminal, usize::MAX), RowCells::default());
}

#[test]
fn soft_wrapped_rows_are_marked() {
    let (mut terminal, _effects) = terminal(5, 3);
    terminal.vt_write(b"abcdefg\r\nx");
    let rows = capture(&terminal, 0).rows;
    let wrapped: Vec<bool> = rows.iter().map(|row| row.wrapped).collect();
    assert_eq!(wrapped, vec![true, false, false]);
}

#[test]
fn ghostsnp_starts_with_its_magic_and_decodes_to_the_same_terminal() {
    let (mut terminal, _effects) = terminal(8, 2);
    terminal.vt_write(b"one\r\ntwo\x1b]2;t\x07");
    let bytes = encode(&mut terminal).unwrap();
    assert_eq!(bytes.get(..8), Some(&b"GHOSTSNP"[..]));
    let restored = decode(&bytes).unwrap();
    assert_eq!(capture(&restored, 10), capture(&terminal, 10));
}

#[test]
fn a_restored_terminal_does_not_borrow_the_snapshot_bytes() {
    let (mut terminal, _effects) = terminal(8, 2);
    terminal.vt_write(b"kept");
    let restored = {
        let bytes = encode(&mut terminal).unwrap();
        decode(&bytes).unwrap()
    };
    assert_eq!(texts(&capture(&restored, 0).rows), vec!["kept", ""]);
}
