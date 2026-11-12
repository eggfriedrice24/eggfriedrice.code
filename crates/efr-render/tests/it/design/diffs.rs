//! The lines of `diff_lines` against TM-Diffs 1 to 3 and the diff rows of TM-Legend
//! (20, 21): the sign bold in its role; in truecolor the file's syntax colours inside
//! and the row of a changed line on its tint; in 16 colours the whole line in the
//! role's colour, no syntax, no background; without colour a removed line dim; a hunk
//! header whole in `diff.hunk`.
//!
//! The CLI lays the lines out (the indent, the sign, one space, the content, the fill;
//! the short form of a hunk header, the cut, the `… N more lines` row); a test here
//! lays them out the same way to compare the cells with the board.

use efr_render::{DiffLine, DiffLineKind, diff_lines};
use efr_test_support::design::{Depth, Frame, assert_rows, cells};

use super::{assert_code_rows, code_options};

/// The first edit of TM-Diffs 1, as a unified diff of the file.
const TURN_DIFF: &str = "--- a/crates/efr-daemon/src/turn.rs\n\
+++ b/crates/efr-daemon/src/turn.rs\n\
@@ -88,4 +88,8 @@ impl Turn\n     \
pub fn finish(&mut self) -> usize {\n         \
let mut drained = 0;\n\
-        self.state = State::Done;\n\
+        while let Ok(msg) = self.rx.try_recv() {\n\
+            self.pending.push(msg);\n\
+            drained += 1;\n\
+        }\n\
+        self.state = State::Done;\n         \
drained\n\
@@ -131,1 +135,1 @@ fn steer\n\
-        self.pending.push(msg);\n\
+        self.tx.send(msg)?;\n";

/// The rows of TM-Diffs 1 to 3 that show the lines of [`TURN_DIFF`] (the hunk headers
/// are in their short form, which the CLI makes).
const TURN_ROWS: [usize; 11] = [4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15];

/// A line laid out as the CLI lays it out under a file edit: `indent`, the sign, one
/// space, the content; a changed line on its fill to the end of the row.
fn lay_out(line: &DiffLine, indent: &str) -> String {
    let fill = line.fill.as_ref().map(|fill| format!("\x1b[{fill}m")).unwrap_or_default();
    let end = if fill.is_empty() { String::new() } else { format!("{fill}\x1b[K\x1b[0m") };
    let row = &line.rows[0];
    match line.kind {
        DiffLineKind::Added | DiffLineKind::Removed | DiffLineKind::Context => {
            format!("{fill}{indent}{}{fill} {row}{end}\n", line.sign)
        }
        _ => format!("{indent}{row}\n"),
    }
}

/// The lines of [`TURN_DIFF`] that the board shows, laid out.
fn turn_rows(depth: Depth) -> String {
    let lines = diff_lines(TURN_DIFF, &code_options(depth, 100));
    lines
        .iter()
        .filter(|line| !matches!(line.kind, DiffLineKind::Hunk | DiffLineKind::Other))
        .map(|line| lay_out(line, "  "))
        .collect()
}

/// TM-Diffs 1: in truecolor a changed row is on `diff.add.bg` or `diff.remove.bg` to
/// the end of the row, with its sign bold in its role and the file's syntax colours
/// inside (as far as a theme rule gives them; see `code`); a context row has no fill.
#[test]
fn the_rows_of_a_diff_in_truecolor_are_the_board() {
    let frame = Frame::load("Diffs", 1).unwrap();
    let rows = cells(&turn_rows(Depth::TrueColor));
    for (row, real) in TURN_ROWS.iter().zip(&rows) {
        let want = &frame.rows[*row];
        assert_code_rows(
            &frame,
            *row..row + 1,
            std::slice::from_ref(real),
            Depth::TrueColor,
            |_| false,
        );
        if let Some(sign) = want.segments.get(1).filter(|segment| segment.look.bold) {
            assert_eq!(real.cells[2].look.fg, sign.look.fg, "the sign of row {row}");
            assert!(real.cells[2].look.bold, "the sign of row {row} is bold");
        }
    }
}

/// TM-Diffs 2 and 3: in 16 colours the whole row in the diff role, no syntax and no
/// background; without colour the sign bold and a removed row dim.
#[test]
fn the_rows_of_a_diff_in_sixteen_colours_and_without_colour_are_the_board() {
    for (number, depth) in [(2, Depth::Ansi16), (3, Depth::NoColor)] {
        let frame = Frame::load("Diffs", number).unwrap();
        let rows = turn_rows(depth);
        let rows: Vec<&str> = rows.split_inclusive('\n').collect();
        for (row, real) in TURN_ROWS.iter().zip(rows) {
            assert_rows(&frame, *row..row + 1, real, depth);
        }
    }
}

/// TM-Legend 20 and 21: the hunk header whole in `diff.hunk` (dim without colour),
/// and an added row: the sign bold, the row on its tint in truecolor.
#[test]
fn the_diff_rows_are_the_legend() {
    let hunk = Frame::load("Legend", 20).unwrap();
    let added = Frame::load("Legend", 21).unwrap();
    for depth in Depth::ALL {
        let options = code_options(depth, hunk.cols);
        let lines = diff_lines("@@ 88 impl Turn\n", &options);
        assert_eq!(lines[0].kind, DiffLineKind::Hunk);
        assert_rows(&hunk, .., &lay_out(&lines[0], "  "), depth);
        let lines = diff_lines("+++ b/src/turn.rs\n@@ -1 +1 @@\n+let x = 1;\n", &options);
        let row = lay_out(&lines[2], "  ");
        if depth == Depth::TrueColor {
            assert_code_rows(&added, 0..1, &cells(&row), depth, |_| false);
        } else {
            // NOTE: in 16 colours the whole row is in the role (TM-Diffs 2), which
            // beats the Legend's syntax colours, drawn for truecolor.
            let rows = cells(&row);
            assert_eq!(rows[0].text(), added.rows[0].text);
            assert!(rows[0].cells[2].look.bold);
            assert_eq!(rows[0].fill, None);
        }
    }
}
