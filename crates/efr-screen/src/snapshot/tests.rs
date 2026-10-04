use efr_protocol::{Cell, RowCells};
use pretty_assertions::assert_eq;

use super::row_text;

fn cell(text: &str) -> Cell {
    Cell { text: text.to_owned(), ..Cell::default() }
}

fn row(cells: Vec<Cell>) -> RowCells {
    RowCells { cells, wrapped: false }
}

fn text_row(text: &str) -> RowCells {
    row(text.chars().map(|c| cell(&c.to_string())).collect())
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
