use efr_protocol::{Cell, Color, Cursor, RowCells, ScreenSnapshot, Size};
use pretty_assertions::assert_eq;

use super::{normalize, row_text};

fn cell(text: &str) -> Cell {
    Cell { text: text.to_owned(), ..Cell::default() }
}

fn row(cells: Vec<Cell>) -> RowCells {
    RowCells { cells, wrapped: false }
}

fn text_row(text: &str) -> RowCells {
    row(text.chars().map(|c| cell(&c.to_string())).collect())
}

fn snapshot(cols: u16, rows: Vec<RowCells>) -> ScreenSnapshot {
    let size = Size { cols, rows: u16::try_from(rows.len()).unwrap() };
    ScreenSnapshot { size, rows, ..ScreenSnapshot::default() }
}

#[test]
fn row_text_renders_blank_cells_as_spaces() {
    let row = row(vec![cell("a"), Cell::default(), cell("b")]);
    assert_eq!(row_text(&row), "a b");
}

#[test]
fn row_text_drops_trailing_spaces() {
    assert_eq!(row_text(&text_row("ls   ")), "ls");
    assert_eq!(row_text(&row(vec![cell("x"), Cell::default(), Cell::default()])), "x");
    assert_eq!(row_text(&RowCells::default()), "");
}

#[test]
fn row_text_skips_the_second_cell_of_a_wide_character() {
    let wide = Cell { text: "\u{4e2d}".to_owned(), wide: true, ..Cell::default() };
    let row = row(vec![wide, Cell::default(), cell("!")]);
    assert_eq!(row_text(&row), "\u{4e2d}!");
}

#[test]
fn normalize_pads_missing_rows() {
    let mut input = snapshot(4, vec![text_row("a")]);
    input.size.rows = 3;
    let output = normalize(input, 0);
    assert_eq!(output.rows, vec![text_row("a"), RowCells::default(), RowCells::default()]);
}

#[test]
fn normalize_drops_rows_below_the_grid() {
    let mut input = snapshot(4, vec![text_row("a"), text_row("b"), text_row("c")]);
    input.size.rows = 2;
    assert_eq!(normalize(input, 0).rows, vec![text_row("a"), text_row("b")]);
}

#[test]
fn normalize_trims_trailing_blank_cells() {
    let input = snapshot(6, vec![row(vec![cell("o"), cell("k"), cell(" "), Cell::default()])]);
    assert_eq!(normalize(input, 0).rows, vec![text_row("ok")]);
}

#[test]
fn normalize_keeps_cells_that_paint_something() {
    let painted = Cell { bg: Some(Color::Indexed(1)), ..Cell::default() };
    let underlined = Cell { text: " ".to_owned(), underline: true, ..Cell::default() };
    let inverse = Cell { inverse: true, ..Cell::default() };
    for kept in [painted, underlined, inverse] {
        let input = snapshot(4, vec![row(vec![cell("a"), kept.clone(), Cell::default()])]);
        assert_eq!(normalize(input, 0).rows, vec![row(vec![cell("a"), kept])]);
    }
}

#[test]
fn normalize_drops_cells_whose_attributes_do_not_show() {
    let bold_fg = Cell { fg: Some(Color::Rgb([1, 2, 3])), bold: true, ..Cell::default() };
    let input = snapshot(4, vec![row(vec![cell("a"), bold_fg])]);
    assert_eq!(normalize(input, 0).rows, vec![text_row("a")]);
}

#[test]
fn normalize_keeps_the_newest_scrollback() {
    let mut input = snapshot(4, vec![text_row("now")]);
    input.scrollback = vec![text_row("1"), text_row("2"), text_row("3 ")];
    let output = normalize(input, 2);
    assert_eq!(output.scrollback, vec![text_row("2"), text_row("3")]);
}

#[test]
fn normalize_keeps_short_scrollback_whole() {
    let mut input = snapshot(4, vec![text_row("now")]);
    input.scrollback = vec![text_row("1")];
    assert_eq!(normalize(input, 10).scrollback, vec![text_row("1")]);
}

#[test]
fn normalize_moves_a_pending_wrap_cursor_onto_the_last_column() {
    let mut input = snapshot(5, vec![text_row("abcde"), RowCells::default()]);
    input.cursor = Cursor { row: 0, col: 5, hidden: false };
    assert_eq!(normalize(input, 0).cursor, Cursor { row: 0, col: 4, hidden: false });
}

#[test]
fn normalize_keeps_the_cursor_inside_the_grid() {
    let mut input = snapshot(5, vec![RowCells::default(), RowCells::default()]);
    input.cursor = Cursor { row: 9, col: 1, hidden: true };
    assert_eq!(normalize(input, 0).cursor, Cursor { row: 1, col: 1, hidden: true });
}

#[test]
fn normalize_leaves_title_and_alternate_screen_alone() {
    let mut input = snapshot(5, vec![RowCells::default()]);
    input.title = Some("vim".to_owned());
    input.alternate_screen = true;
    let output = normalize(input, 0);
    assert_eq!(output.title.as_deref(), Some("vim"));
    assert!(output.alternate_screen);
}

#[test]
fn normalize_handles_an_empty_grid() {
    let input = ScreenSnapshot::default();
    assert_eq!(normalize(input, 0), ScreenSnapshot::default());
}
