use pretty_assertions::assert_eq;

use super::diff_rows;
use crate::options::{ColourMode, RenderOptions};
use crate::width::{WidthMethod, display_width};

const DIFF: &str = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    println!(\"old\");\n+    println!(\"a new line that is much longer than forty columns\");\n }\n";

/// `rows` with escape bytes as `\e`, one row per line and a blank line between the rows
/// of two lines.
fn shown(rows: &[Vec<String>]) -> String {
    rows.iter().map(|line| line.join("\n").replace('\x1b', "\\e")).collect::<Vec<_>>().join("\n\n")
}

/// `painted` without its escape sequences.
fn bare(painted: &str) -> String {
    let mut out = String::new();
    let mut chars = painted.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn a_diff_is_painted_in_the_diff_roles_at_40_and_80_columns() {
    for columns in [40_u16, 80] {
        let rows = diff_rows(DIFF, &RenderOptions::new(columns));
        insta::assert_snapshot!(format!("colour_{columns}"), shown(&rows));
        let plain = diff_rows(DIFF, &RenderOptions::new(columns).with_colour(ColourMode::None));
        insta::assert_snapshot!(format!("no_colour_{columns}"), shown(&plain));
    }
}

#[test]
fn every_row_fits_and_the_rows_of_a_line_read_back_as_the_line() {
    for columns in [12_u16, 40, 80] {
        let options = RenderOptions::new(columns).with_colour(ColourMode::None);
        let rows = diff_rows(DIFF, &options);
        assert_eq!(rows.len(), DIFF.lines().count());
        for (line, rows) in DIFF.lines().zip(&rows) {
            for row in rows {
                assert!(display_width(row, WidthMethod::CodePoint) <= usize::from(columns));
            }
            let joined: String = rows.iter().map(|row| bare(row)).collect();
            assert_eq!(joined.trim_end(), line.trim_end(), "{columns}");
        }
    }
}

#[test]
fn a_cut_keeps_the_spaces_before_it() {
    let options = RenderOptions::new(6).with_colour(ColourMode::None);
    let rows = diff_rows("+ab   cd\n", &options);
    assert_eq!(rows[0].iter().map(|row| bare(row)).collect::<Vec<_>>(), ["+ab   ", "cd"]);
}

#[test]
fn a_wide_character_never_sticks_out_of_its_row() {
    let options = RenderOptions::new(3).with_colour(ColourMode::None);
    let rows = diff_rows("+a\u{4e2d}\u{6587}\n", &options);
    let bare_rows: Vec<String> = rows[0].iter().map(|row| bare(row)).collect();
    assert_eq!(bare_rows, ["+a", "\u{4e2d}", "\u{6587}"]);
}

#[test]
fn without_a_terminal_each_line_is_one_plain_row() {
    let options = RenderOptions::new(10).with_terminal(false);
    let rows = diff_rows("@@ -1 +1 @@\n-old\x1b[31m\n+new\tline\n", &options);
    assert_eq!(rows, [vec!["@@ -1 +1 @@"], vec!["-old\u{241b}[31m"], vec!["+new    line"]]);
}
