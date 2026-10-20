use pretty_assertions::assert_eq;

use super::*;

/// A vt100 screen of 2 rows by `cols` columns after `bytes`.
fn screen(cols: u16, bytes: &[u8]) -> vt100::Parser {
    let mut parser = vt100::Parser::new(2, cols, 0);
    parser.process(bytes);
    parser
}

/// The first cell of the top row after `bytes`.
fn first_cell(bytes: &[u8]) -> Cell {
    let parser = screen(10, bytes);
    row(parser.screen(), 0).cells.swap_remove(0)
}

fn text(value: &str) -> Cell {
    Cell { text: value.to_owned(), ..Cell::default() }
}

#[test]
fn a_plain_character_has_default_attributes() {
    assert_eq!(first_cell(b"a"), text("a"));
}

#[test]
fn an_erased_cell_has_no_text() {
    assert_eq!(first_cell(b"a\x1b[2J"), Cell::default());
}

#[test]
fn every_row_has_a_cell_per_column() {
    let parser = screen(7, b"ab");
    let top = row(parser.screen(), 0);
    assert_eq!(top.cells.len(), 7);
    assert_eq!(top.cells[2..], vec![Cell::default(); 5]);
}

#[test]
fn colours_map_to_the_wire_colours() {
    assert_eq!(color(vt100::Color::Default), None);
    assert_eq!(color(vt100::Color::Idx(9)), Some(Color::Indexed(9)));
    assert_eq!(color(vt100::Color::Rgb(1, 2, 3)), Some(Color::Rgb([1, 2, 3])));
}

#[test]
fn sgr_colours_reach_the_cell() {
    let expected = |fg, bg| Cell { fg, bg, ..text("x") };
    assert_eq!(
        first_cell(b"\x1b[31;42mx"),
        expected(Some(Color::Indexed(1)), Some(Color::Indexed(2)))
    );
    assert_eq!(first_cell(b"\x1b[91mx"), expected(Some(Color::Indexed(9)), None));
    assert_eq!(first_cell(b"\x1b[38;5;200mx"), expected(Some(Color::Indexed(200)), None));
    assert_eq!(
        first_cell(b"\x1b[38;2;1;2;3;48;2;4;5;6mx"),
        expected(Some(Color::Rgb([1, 2, 3])), Some(Color::Rgb([4, 5, 6])))
    );
}

#[test]
fn sgr_attributes_reach_the_cell() {
    assert_eq!(first_cell(b"\x1b[1mx"), Cell { bold: true, ..text("x") });
    assert_eq!(first_cell(b"\x1b[3mx"), Cell { italic: true, ..text("x") });
    assert_eq!(first_cell(b"\x1b[4mx"), Cell { underline: true, ..text("x") });
    assert_eq!(first_cell(b"\x1b[7mx"), Cell { inverse: true, ..text("x") });
    assert_eq!(first_cell(b"\x1b[1;3;4;7m\x1b[0mx"), text("x"));
}

#[test]
fn dim_has_no_wire_field() {
    assert_eq!(first_cell(b"\x1b[2mx"), text("x"));
}

#[test]
fn a_wide_character_marks_its_first_cell_and_leaves_the_second_empty() {
    let parser = screen(10, "\u{4e2d}b".as_bytes());
    let cells = row(parser.screen(), 0).cells;
    assert_eq!(cells[0], Cell { wide: true, ..text("\u{4e2d}") });
    assert_eq!(cells[1], Cell::default());
    assert_eq!(cells[2], text("b"));
}

#[test]
fn a_combining_mark_stays_in_its_base_cell() {
    let parser = screen(10, "e\u{301}x".as_bytes());
    let cells = row(parser.screen(), 0).cells;
    assert_eq!(cells[0], text("e\u{301}"));
    assert_eq!(cells[1], text("x"));
}

#[test]
fn a_soft_wrapped_row_says_so() {
    let parser = screen(5, b"abcdefg");
    assert!(row(parser.screen(), 0).wrapped);
    assert!(!row(parser.screen(), 1).wrapped);
}

#[test]
fn a_row_ended_by_a_line_feed_is_not_wrapped() {
    let parser = screen(5, b"abcde\r\nfg");
    assert!(!row(parser.screen(), 0).wrapped);
}
