use efr_protocol::{Cell, Cursor, RowCells, ScreenSnapshot, Size};
use pretty_assertions::assert_eq;

use super::ScreenText;

fn row(text: &str) -> RowCells {
    let cells = text.chars().map(|c| Cell { text: c.to_string(), ..Cell::default() }).collect();
    RowCells { cells, wrapped: false }
}

fn wrapped(text: &str) -> RowCells {
    RowCells { wrapped: true, ..row(text) }
}

fn snapshot(scrollback: Vec<RowCells>, rows: Vec<RowCells>, cursor_row: u16) -> ScreenSnapshot {
    ScreenSnapshot {
        size: Size { cols: 4, rows: 4 },
        cursor: Cursor { row: cursor_row, col: 0, hidden: false },
        rows,
        scrollback,
        ..ScreenSnapshot::default()
    }
}

fn text(text: &str, blank_lines_before_cursor: usize) -> ScreenText {
    ScreenText { text: text.to_owned(), blank_lines_before_cursor }
}

#[test]
fn the_scrollback_comes_first_and_trailing_blanks_go() {
    let screen = snapshot(vec![row("ab  ")], vec![row("c"), row(""), row("  d "), row("")], 3);
    assert_eq!(ScreenText::of(&screen), text("ab\nc\n\n  d", 0));
}

#[test]
fn blank_lines_above_the_cursor_are_counted_apart() {
    let screen = snapshot(Vec::new(), vec![row("a"), row(""), row(""), row("")], 2);
    assert_eq!(ScreenText::of(&screen), text("a", 1));
    let blank = snapshot(Vec::new(), vec![row(""), row(""), row(""), row("")], 1);
    assert_eq!(ScreenText::of(&blank), text("", 1));
    let below = snapshot(Vec::new(), vec![row(""), row("a"), row(""), row("")], 0);
    assert_eq!(ScreenText::of(&below), text("\na", 0));
}

#[test]
fn a_wrapped_row_keeps_its_blank_at_the_wrap() {
    let screen = snapshot(Vec::new(), vec![wrapped("ab"), row("c"), row(""), row("")], 2);
    assert_eq!(ScreenText::of(&screen), text("ab  c", 0));
}

#[test]
fn a_wide_character_at_the_end_of_a_wrapped_row_fills_it() {
    let mut first = wrapped("ab");
    first.cells.push(Cell { text: "日".to_owned(), wide: true, ..Cell::default() });
    let screen = snapshot(Vec::new(), vec![first, row("c"), row(""), row("")], 2);
    assert_eq!(ScreenText::of(&screen).text, "ab日c");
    let mut spacer = wrapped("abc");
    let mut wide = row("");
    wide.cells.push(Cell { text: "日".to_owned(), wide: true, ..Cell::default() });
    let screen = snapshot(Vec::new(), vec![spacer.clone(), wide.clone(), row(""), row("")], 2);
    assert_eq!(ScreenText::of(&screen).text, "abc日");
    spacer.cells.truncate(2);
    let screen = snapshot(Vec::new(), vec![spacer, wide, row(""), row("")], 2);
    assert_eq!(ScreenText::of(&screen).text, "ab 日");
}
