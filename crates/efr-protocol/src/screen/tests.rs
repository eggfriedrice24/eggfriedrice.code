use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{Cell, Color, Cursor, RowCells, ScreenSnapshot, Size};

fn plain(text: &str) -> Cell {
    Cell { text: text.into(), ..Cell::default() }
}

#[test]
fn a_plain_cell_is_only_its_text() {
    assert_eq!(serde_json::to_value(plain("a")).unwrap(), json!({ "text": "a" }));
}

#[test]
fn set_attributes_and_colours_appear_on_the_wire() {
    let cell = Cell {
        text: "x".into(),
        fg: Some(Color::Indexed(1)),
        bg: Some(Color::Rgb([255, 128, 0])),
        bold: true,
        wide: true,
        ..Cell::default()
    };
    assert_eq!(
        serde_json::to_value(&cell).unwrap(),
        json!({
            "text": "x",
            "fg": { "indexed": 1 },
            "bg": { "rgb": [255, 128, 0] },
            "bold": true,
            "wide": true,
        })
    );
}

#[test]
fn missing_attributes_read_as_off() {
    let cell: Cell = serde_json::from_value(json!({ "text": "a" })).unwrap();
    assert_eq!(cell, plain("a"));
}

#[test]
fn a_visible_cursor_has_no_hidden_member() {
    let cursor = Cursor { row: 2, col: 5, hidden: false };
    assert_eq!(serde_json::to_value(cursor).unwrap(), json!({ "row": 2, "col": 5 }));
}

#[test]
fn an_empty_scrollback_and_the_main_screen_are_left_out() {
    let snapshot = ScreenSnapshot {
        size: Size { cols: 2, rows: 1 },
        cursor: Cursor::default(),
        rows: vec![RowCells { cells: vec![plain("$")], wrapped: false }],
        ..ScreenSnapshot::default()
    };
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        json!({
            "size": { "cols": 2, "rows": 1 },
            "cursor": { "row": 0, "col": 0 },
            "rows": [{ "cells": [{ "text": "$" }] }],
        })
    );
}

#[test]
fn a_full_snapshot_reads_back_as_itself() {
    let snapshot = ScreenSnapshot {
        size: Size { cols: 80, rows: 2 },
        cursor: Cursor { row: 1, col: 2, hidden: true },
        rows: vec![
            RowCells { cells: vec![plain("a"), plain("b")], wrapped: true },
            RowCells { cells: vec![plain("c")], wrapped: false },
        ],
        scrollback: vec![RowCells { cells: vec![plain("old")], wrapped: false }],
        title: Some("vim".into()),
        alternate_screen: true,
    };
    let back: ScreenSnapshot =
        serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
    assert_eq!(back, snapshot);
}
