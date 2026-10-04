//! Snapshots in the wire form, and the text a row shows.

use efr_protocol::{RowCells, ScreenSnapshot, Seq};

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

#[cfg(test)]
mod tests;
