//! What a ghostty screen shows, in two forms.
//!
//! - The wire form, `efr_protocol::ScreenSnapshot`, which every client can render:
//!   the visible rows, the newest rows of scrollback, the cursor, the title and
//!   whether the alternate screen is up. Cells are read one at a time through grid
//!   references, which is slow for a render loop but fine for an attach or a test.
//! - GHOSTSNP, libghostty-vt's own snapshot: an authenticated record stream of the
//!   whole terminal, parser state included, that another libghostty-vt restores into
//!   an identical terminal. It is the attach format for a client that runs
//!   libghostty-vt itself.

mod cells;

use efr_screen::{Cursor, RowCells, ScreenSnapshot, Size};
use libghostty_vt::Terminal;
use libghostty_vt::screen::Screen as ActiveScreen;
use libghostty_vt::snapshot::Decoder;
use libghostty_vt::terminal::{Point, PointCoordinate};

use crate::GhosttyError;

/// The visible grid, `scrollback_rows` rows of scrollback (fewer when the terminal
/// holds fewer), the cursor, the title and the screen in use.
pub(crate) fn capture(terminal: &Terminal<'_, '_>, scrollback_rows: usize) -> ScreenSnapshot {
    let size = size(terminal);
    let history = terminal.scrollback_rows().unwrap_or(0);
    let scrollback = (history.saturating_sub(scrollback_rows)..history)
        .filter_map(|y| u32::try_from(y).ok())
        .map(|y| cells::row(terminal, size.cols, |x| Point::History(PointCoordinate { x, y })))
        .collect();
    let rows = (0..size.rows).map(|y| visible_row(terminal, size.cols, y)).collect();
    ScreenSnapshot {
        size,
        cursor: cursor(terminal),
        rows,
        scrollback,
        title: title(terminal).map(str::to_owned),
        alternate_screen: terminal
            .active_screen()
            .is_ok_and(|screen| screen == ActiveScreen::Alternate),
    }
}

/// Row `index` of the visible grid; an empty row outside it.
pub(crate) fn active_row(terminal: &Terminal<'_, '_>, index: usize) -> RowCells {
    let size = size(terminal);
    match u16::try_from(index) {
        Ok(y) if y < size.rows => visible_row(terminal, size.cols, y),
        _ => RowCells::default(),
    }
}

/// The cursor in the visible grid. libghostty-vt keeps a cursor with a pending wrap
/// on the last column, so it is always inside the grid.
pub(crate) fn cursor(terminal: &Terminal<'_, '_>) -> Cursor {
    Cursor {
        row: terminal.cursor_y().unwrap_or(0),
        col: terminal.cursor_x().unwrap_or(0),
        hidden: !terminal.is_cursor_visible().unwrap_or(true),
    }
}

/// The title the program set; libghostty-vt reports "no title" as an empty string.
pub(crate) fn title<'t>(terminal: &'t Terminal<'_, '_>) -> Option<&'t str> {
    terminal.title().ok().filter(|title| !title.is_empty())
}

/// The whole terminal as a GHOSTSNP record stream.
pub(crate) fn encode(terminal: &mut Terminal<'_, '_>) -> Result<Vec<u8>, GhosttyError> {
    let mut bytes = Vec::new();
    terminal
        .encode_snapshot(&mut bytes)
        .map_err(|source| GhosttyError::EncodeSnapshot { source })?;
    Ok(bytes)
}

/// A terminal restored from a GHOSTSNP record stream, its scrollback included. The
/// terminal does not borrow `bytes`.
pub(crate) fn decode(bytes: &[u8]) -> Result<Terminal<'static, 'static>, GhosttyError> {
    let decode = |source| GhosttyError::DecodeSnapshot { source };
    Decoder::new_buf(bytes).and_then(Decoder::decode).map_err(decode)
}

fn size(terminal: &Terminal<'_, '_>) -> Size {
    Size { cols: terminal.cols().unwrap_or(0), rows: terminal.rows().unwrap_or(0) }
}

fn visible_row(terminal: &Terminal<'_, '_>, cols: u16, y: u16) -> RowCells {
    cells::row(terminal, cols, |x| Point::Active(PointCoordinate { x, y: u32::from(y) }))
}

#[cfg(test)]
mod tests;
