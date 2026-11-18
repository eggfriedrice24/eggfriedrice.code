//! What a screen shows, as lines of text.
//!
//! A reader that wants words, not cells (the replay of a command's output in
//! `efr-shell`), asks a screen for [`ScreenText`] through
//! [`ScreenHandle::text`](crate::ScreenHandle::text). A backend makes it from its
//! snapshot through [`ScreenText::of`]. A backend that counts its scrollback rows
//! without reading them (both do) reads no row when the scrollback is full.

use efr_protocol::{RowCells, ScreenSnapshot};

use crate::row_text;
use crate::snapshot::is_blank;

/// The scrollback and the visible rows of a screen as lines of text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenText {
    /// The lines, oldest first, joined with `\n`: a soft-wrapped row is joined to the
    /// next row with every column, so a blank at the wrap stays; a line has no
    /// trailing spaces; blank lines at the start and in the middle stay, and there is
    /// no blank line at the end.
    pub text: String,
    /// The blank lines that start above the cursor's row after the last line with
    /// text: where more output goes on. They are not in [`text`](Self::text).
    pub blank_lines_before_cursor: usize,
}

impl ScreenText {
    /// The text of a snapshot: its scrollback, then its visible rows. The snapshot
    /// may be in the wire form, as
    /// [`ScreenHandle::snapshot`](crate::ScreenHandle::snapshot) gives it, or as a
    /// backend builds it, with trailing blank cells.
    pub fn of(snapshot: &ScreenSnapshot) -> ScreenText {
        let rows: Vec<&RowCells> = snapshot.scrollback.iter().chain(&snapshot.rows).collect();
        let cols = usize::from(snapshot.size.cols);
        let cursor = snapshot.scrollback.len() + usize::from(snapshot.cursor.row);
        // Each line with the index of its first row.
        let mut lines: Vec<(usize, String)> = Vec::new();
        let mut open: Option<(usize, String)> = None;
        for (index, row) in rows.iter().enumerate() {
            let (start, mut text) = open.take().unwrap_or_else(|| (index, String::new()));
            match rows.get(index + 1) {
                Some(next) if row.wrapped => {
                    text.push_str(&wrapped_text(row, cols, next));
                    open = Some((start, text));
                }
                _ => {
                    text.push_str(&row_text(row));
                    text.truncate(text.trim_end_matches(' ').len());
                    lines.push((start, text));
                }
            }
        }
        let mut blank_lines_before_cursor = 0;
        while let Some((start, _)) = lines.last().filter(|(_, text)| text.is_empty()) {
            if *start < cursor {
                blank_lines_before_cursor += 1;
            }
            lines.pop();
        }
        let text = lines.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n");
        ScreenText { text, blank_lines_before_cursor }
    }
}

/// The text of a soft-wrapped row: every column, because the line goes on in the next
/// row and a blank at the wrap is part of it. The trailing blank cells come back as
/// spaces, except the one that a wide character leaves in the last column when it
/// does not fit there and starts the next row instead.
fn wrapped_text(row: &RowCells, cols: usize, next: &RowCells) -> String {
    // NOTE: the wire form leaves trailing blank cells out and a backend's own
    // snapshot may not, so they are cut here first; then both read the same.
    let cells =
        &row.cells[..row.cells.iter().rposition(|cell| !is_blank(cell)).map_or(0, |last| last + 1)];
    let mut text = String::new();
    let mut after_wide = false;
    for cell in cells {
        if cell.text.is_empty() {
            if !after_wide {
                text.push(' ');
            }
        } else {
            text.push_str(&cell.text);
        }
        after_wide = cell.wide;
    }
    // A wide character in the last cells lost its blank second cell to the trim.
    let covered = cells.len() + usize::from(cells.last().is_some_and(|cell| cell.wide));
    let mut blanks = cols.saturating_sub(covered);
    if blanks > 0 && next.cells.first().is_some_and(|cell| cell.wide) {
        blanks -= 1;
    }
    text.extend(std::iter::repeat_n(' ', blanks));
    text
}

#[cfg(test)]
mod tests;
