//! From a backend's snapshot to the wire snapshot.
//!
//! Backends build an `efr_protocol::ScreenSnapshot` in their own way: vt100 pads every
//! row to the full width, libghostty-vt reports a pending-wrap cursor one column past
//! the edge, and either may hand back more scrollback than was asked for. The actor
//! runs every snapshot through [`normalize`] before it leaves the screen thread, so
//! clients and the conformance suite see one shape whatever the backend.

use efr_protocol::{Cell, RowCells, ScreenSnapshot, Seq};

/// A snapshot together with the stream position it shows.
///
/// `at` is the recording offset just after the last byte the screen processed, so a
/// client that attaches with this snapshot continues with the recording from `at`
/// and misses nothing. It is `Seq::ZERO` before the first feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenCapture {
    /// The recording offset after the last byte fed before the snapshot.
    pub at: Seq,
    /// The screen at that moment, normalised.
    pub snapshot: ScreenSnapshot,
}

/// The text a row shows: each cell's grapheme, a space for a blank cell, nothing for
/// the second cell of a wide character, and no trailing spaces.
pub fn row_text(row: &RowCells) -> String {
    let mut text = String::new();
    let mut after_wide = false;
    for cell in &row.cells {
        if cell.text.is_empty() {
            if !after_wide {
                text.push(' ');
            }
        } else {
            text.push_str(&cell.text);
        }
        after_wide = cell.wide;
    }
    text.truncate(text.trim_end_matches(' ').len());
    text
}

/// Shapes a backend's snapshot into the wire form:
///
/// - exactly `size.rows` visible rows (missing rows are blank, extra rows are dropped
///   from the bottom);
/// - at most `scrollback_rows` rows of scrollback, the newest kept;
/// - no trailing blank cells in any row, which the wire format allows to be left out;
/// - the cursor inside the grid, so a pending-wrap cursor sits on the last column.
pub(crate) fn normalize(mut snapshot: ScreenSnapshot, scrollback_rows: usize) -> ScreenSnapshot {
    let rows = usize::from(snapshot.size.rows);
    snapshot.rows.resize_with(rows, RowCells::default);
    let excess = snapshot.scrollback.len().saturating_sub(scrollback_rows);
    snapshot.scrollback.drain(..excess);
    for row in snapshot.rows.iter_mut().chain(snapshot.scrollback.iter_mut()) {
        trim_trailing_blanks(row);
    }
    let cursor = &mut snapshot.cursor;
    cursor.row = cursor.row.min(snapshot.size.rows.saturating_sub(1));
    cursor.col = cursor.col.min(snapshot.size.cols.saturating_sub(1));
    snapshot
}

fn trim_trailing_blanks(row: &mut RowCells) {
    let keep = row.cells.iter().rposition(|cell| !is_blank(cell)).map_or(0, |last| last + 1);
    row.cells.truncate(keep);
}

/// A cell that shows nothing: no glyph and nothing that paints the cell itself. A
/// foreground colour, bold or italic on an empty cell does not show.
fn is_blank(cell: &Cell) -> bool {
    (cell.text.is_empty() || cell.text == " ")
        && cell.bg.is_none()
        && !cell.underline
        && !cell.inverse
        && !cell.wide
}

#[cfg(test)]
mod tests;
