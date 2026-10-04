use efr_screen::{Cell, Color};
use libghostty_vt::Terminal;
use libghostty_vt::terminal::{Point, PointCoordinate};
use pretty_assertions::assert_eq;

use super::{cell, row};
use crate::testing::{terminal, texts};

fn cells(terminal: &Terminal<'_, '_>, y: u32) -> Vec<Cell> {
    let cols = terminal.cols().unwrap();
    row(terminal, cols, |x| Point::Active(PointCoordinate { x, y })).cells
}

fn text(content: &str) -> Cell {
    Cell { text: content.to_owned(), ..Cell::default() }
}

#[test]
fn plain_text_has_one_grapheme_per_cell_and_blanks_after() {
    let (mut terminal, _effects) = terminal(4, 1);
    terminal.vt_write(b"ab");
    assert_eq!(cells(&terminal, 0), vec![text("a"), text("b"), Cell::default(), Cell::default()]);
}

#[test]
fn sgr_attributes_map_to_the_wire_cell() {
    let (mut terminal, _effects) = terminal(6, 1);
    terminal
        .vt_write(b"\x1b[1;3;4;7;31;42mA\x1b[0m\x1b[38;2;1;2;3mB\x1b[0m\x1b[38;5;200;48;5;17mC");
    let row = cells(&terminal, 0);
    assert_eq!(
        row[0],
        Cell {
            text: "A".to_owned(),
            fg: Some(Color::Indexed(1)),
            bg: Some(Color::Indexed(2)),
            bold: true,
            italic: true,
            underline: true,
            inverse: true,
            wide: false,
        }
    );
    assert_eq!(row[1], Cell { fg: Some(Color::Rgb([1, 2, 3])), ..text("B") });
    assert_eq!(
        row[2],
        Cell { fg: Some(Color::Indexed(200)), bg: Some(Color::Indexed(17)), ..text("C") }
    );
}

#[test]
fn curly_and_double_underlines_are_underlines() {
    let (mut terminal, _effects) = terminal(4, 1);
    terminal.vt_write(b"\x1b[4:3mx\x1b[0m\x1b[21my");
    let row = cells(&terminal, 0);
    assert!(row[0].underline);
    assert!(row[1].underline);
}

#[test]
fn cells_erased_with_a_background_keep_it() {
    let (mut terminal, _effects) = terminal(4, 1);
    terminal.vt_write(b"\x1b[41m\x1b[K\x1b[0m");
    let blank_red = Cell { bg: Some(Color::Indexed(1)), ..Cell::default() };
    assert_eq!(cells(&terminal, 0), vec![blank_red; 4]);
    terminal.vt_write(b"\x1b[48;2;9;8;7m\x1b[2K");
    let blank_rgb = Cell { bg: Some(Color::Rgb([9, 8, 7])), ..Cell::default() };
    assert_eq!(cells(&terminal, 0), vec![blank_rgb; 4]);
}

#[test]
fn a_wide_character_takes_two_cells() {
    let (mut terminal, _effects) = terminal(4, 1);
    terminal.vt_write("\u{4e2d}x".as_bytes());
    assert_eq!(
        cells(&terminal, 0),
        vec![Cell { wide: true, ..text("\u{4e2d}") }, Cell::default(), text("x"), Cell::default()]
    );
}

#[test]
fn the_spacer_at_the_end_of_a_wrapped_row_is_blank() {
    let (mut terminal, _effects) = terminal(3, 2);
    terminal.vt_write("ab\u{4e2d}".as_bytes());
    assert_eq!(
        texts(&[row(&terminal, 3, |x| Point::Active(PointCoordinate { x, y: 0 }))]),
        vec!["ab"]
    );
    assert_eq!(cells(&terminal, 0)[2], Cell::default());
    assert_eq!(cells(&terminal, 1)[0], Cell { wide: true, ..text("\u{4e2d}") });
}

#[test]
fn combining_marks_stay_with_their_base() {
    let (mut terminal, _effects) = terminal(4, 1);
    terminal.vt_write("e\u{301}x".as_bytes());
    let row = cells(&terminal, 0);
    assert_eq!(row[0], text("e\u{301}"));
    assert_eq!(row[1], text("x"));
}

#[test]
fn a_long_grapheme_cluster_is_read_whole() {
    let (mut terminal, _effects) = terminal(4, 1);
    let cluster: String =
        std::iter::once('e').chain((0x301..0x313).filter_map(char::from_u32)).collect();
    terminal.vt_write(cluster.as_bytes());
    let grid = terminal.grid_ref(Point::Active(PointCoordinate { x: 0, y: 0 })).unwrap();
    assert_eq!(cell(&grid).text, cluster);
}
