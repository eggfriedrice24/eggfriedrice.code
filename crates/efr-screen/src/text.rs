//! What a screen shows, as lines of text.
//!
//! A reader that wants words, not cells (the replay of a command's output in
//! `efr-shell`), asks a screen for [`ScreenText`] through
//! [`ScreenHandle::text`](crate::ScreenHandle::text). A backend makes it from its
//! snapshot through [`ScreenText::of`]. A backend counts its scrollback rows without
//! reading them, so it reads no row when the scrollback is full.

use efr_protocol::{RowCells, ScreenSnapshot};

use crate::snapshot::push_cells;

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
    /// must be in the wire form, as
    /// [`ScreenHandle::snapshot`](crate::ScreenHandle::snapshot) gives it, with no
    /// trailing blank cells in a row.
    pub fn of(snapshot: &ScreenSnapshot) -> ScreenText {
        let cols = usize::from(snapshot.size.cols);
        let cursor = snapshot.scrollback.len() + usize::from(snapshot.cursor.row);
        let mut text = String::new();
        // Each line: the index of its first row and the byte of `text` where it starts.
        let mut lines: Vec<(usize, usize)> = Vec::new();
        let mut open = false;
        let mut rows = snapshot.scrollback.iter().chain(&snapshot.rows).enumerate().peekable();
        while let Some((index, row)) = rows.next() {
            if !open {
                if !lines.is_empty() {
                    text.push('\n');
                }
                lines.push((index, text.len()));
            }
            open = match rows.peek() {
                Some((_, next)) if row.wrapped => {
                    push_wrapped(&mut text, row, cols, next);
                    true
                }
                _ => {
                    push_cells(&mut text, &row.cells);
                    // The cut stops at the `\n` before the line or at the start.
                    text.truncate(text.trim_end_matches(' ').len());
                    false
                }
            };
        }
        let mut blank_lines_before_cursor = 0;
        while let Some(&(first_row, start)) = lines.last().filter(|(_, start)| *start == text.len())
        {
            if first_row < cursor {
                blank_lines_before_cursor += 1;
            }
            lines.pop();
            // The blank line goes with the `\n` before it.
            text.truncate(start.saturating_sub(1));
        }
        ScreenText { text, blank_lines_before_cursor }
    }
}

/// Appends a soft-wrapped row: every column, because the line goes on in the next row
/// and a blank at the wrap is part of it. The trailing blank cells, which the wire
/// form leaves out, come back as spaces, except the one that a wide character leaves
/// in the last column when it does not fit there and starts the next row instead.
fn push_wrapped(text: &mut String, row: &RowCells, cols: usize, next: &RowCells) {
    push_cells(text, &row.cells);
    // A wide character in the last cells lost its blank second cell to the trim.
    let covered = row.cells.len() + usize::from(row.cells.last().is_some_and(|cell| cell.wide));
    let mut blanks = cols.saturating_sub(covered);
    if blanks > 0 && next.cells.first().is_some_and(|cell| cell.wide) {
        blanks -= 1;
    }
    text.extend(std::iter::repeat_n(' ', blanks));
}

#[cfg(test)]
mod tests;
