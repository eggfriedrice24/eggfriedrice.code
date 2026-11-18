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

use efr_screen::{Cursor, RowCells, ScreenSnapshot, ScreenText, Size};
use libghostty_vt::Terminal;
use libghostty_vt::fmt::{Format, Formatter};
use libghostty_vt::point::{Coordinate, Point};
use libghostty_vt::screen::Screen as ActiveScreen;
use libghostty_vt::snapshot::Decoder;

use crate::{GhosttyError, ffi};

/// The longest unfinished escape sequence kept for a GHOSTSNP snapshot. A snapshot
/// taken while the parser is inside a longer one (a large DCS image) fails until
/// the sequence ends; ordinary sequences are far shorter.
pub(crate) const CONTINUATION_LIMIT: usize = 1 << 20;

/// The visible grid, `scrollback_rows` rows of scrollback (fewer when the terminal
/// holds fewer), the cursor, the title and the screen in use.
pub(crate) fn capture(terminal: &Terminal<'_, '_>, scrollback_rows: usize) -> ScreenSnapshot {
    let size = size(terminal);
    let history = terminal.scrollback_rows().unwrap_or(0);
    let scrollback = (history.saturating_sub(scrollback_rows)..history)
        .filter_map(|y| u32::try_from(y).ok())
        .map(|y| cells::row(terminal, size.cols, |x| Point::History(Coordinate { x, y })))
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

/// The text of the scrollback and the visible rows from libghostty-vt's formatter in
/// one call: plain text, a soft-wrapped row joined to the next, no trailing spaces and
/// no blank line at the end, which is the text that Ghostty copies to the clipboard.
/// It follows the rules of `efr_screen::ScreenText::of` and reads no cells except for
/// the blank rows next to the cursor.
pub(crate) fn text(terminal: &Terminal<'_, '_>) -> Result<ScreenText, libghostty_vt::Error> {
    let mut formatter = Formatter::new(terminal)?;
    formatter.set_format(Format::Plain)?.set_unwrap(true)?.set_trim(true)?;
    let bytes = formatter.format_alloc(None)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // NOTE: the formatter removes the blank rows at the end, but a row of spaces that
    // the program printed is not blank to it: it gives an empty line there.
    text.truncate(text.trim_end_matches('\n').len());
    Ok(ScreenText { text, blank_lines_before_cursor: blank_lines_before_cursor(terminal) })
}

/// The blank lines that start above the cursor's row after the last line with text,
/// which the formatter leaves out. None when a row at or below the cursor has text.
fn blank_lines_before_cursor(terminal: &Terminal<'_, '_>) -> usize {
    let cols = terminal.cols().unwrap_or(0);
    let history = terminal.scrollback_rows().unwrap_or(0);
    let cursor = terminal.cursor_y().unwrap_or(0);
    let point = |row: usize| {
        move |x| match row.checked_sub(history).and_then(|y| u32::try_from(y).ok()) {
            Some(y) => Point::Active(Coordinate { x, y }),
            None => Point::History(Coordinate { x, y: u32::try_from(row).unwrap_or(u32::MAX) }),
        }
    };
    let below = history + usize::from(cursor)..history + usize::from(terminal.rows().unwrap_or(0));
    if below.into_iter().any(|row| !cells::blank(terminal, cols, point(row))) {
        return 0;
    }
    let mut blank = 0;
    for row in (0..history + usize::from(cursor)).rev() {
        if !cells::blank(terminal, cols, point(row)) {
            break;
        }
        // A blank row starts a line of its own unless the row above wraps into it.
        if row == 0 || !cells::wraps(terminal, point(row - 1)) {
            blank += 1;
        }
    }
    blank
}

/// The cursor in the visible grid. libghostty-vt keeps a cursor with a pending wrap
/// on the last column, so it is always inside the grid.
fn cursor(terminal: &Terminal<'_, '_>) -> Cursor {
    Cursor {
        row: terminal.cursor_y().unwrap_or(0),
        col: terminal.cursor_x().unwrap_or(0),
        hidden: !terminal.is_cursor_visible().unwrap_or(true),
    }
}

/// The title the program set; libghostty-vt reports "no title" as an empty string.
/// A title that is not UTF-8 shows as no title.
fn title<'t>(terminal: &'t Terminal<'_, '_>) -> Option<&'t str> {
    terminal.title().ok().and_then(title_text).filter(|title| !title.is_empty())
}

/// The bytes of a title as text. libghostty-vt cuts a title at 1024 bytes, also
/// inside a character, so a character that is cut at the end is removed and the
/// rest stays. Bytes that are not UTF-8 before the end give `None`.
pub(crate) fn title_text(bytes: &[u8]) -> Option<&str> {
    match str::from_utf8(bytes) {
        Ok(text) => Some(text),
        // NOTE: no `error_len` means that the bytes end inside a character.
        Err(error) if error.error_len().is_none() => {
            bytes.get(..error.valid_up_to()).and_then(|valid| str::from_utf8(valid).ok())
        }
        Err(_) => None,
    }
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
///
/// The restored terminal keeps tracking the unfinished escape sequence of the
/// snapshot up to [`CONTINUATION_LIMIT`], so a snapshot of it is possible at once.
/// A snapshot with a longer unfinished sequence is refused.
pub(crate) fn decode(bytes: &[u8]) -> Result<Terminal<'static, 'static>, GhosttyError> {
    ffi::init();
    let decode = |source| GhosttyError::DecodeSnapshot { source };
    let mut decoder = Decoder::new_buf(bytes).map_err(decode)?;
    // NOTE: the tracking must be on before the decode. Turned on after it, while the
    // parser is inside a sequence, it loses that sequence, and the next encode fails
    // until the sequence ends.
    decoder
        .set_max_continuation_bytes(CONTINUATION_LIMIT)
        .and_then(|decoder| decoder.set_retain_continuation(true))
        .map_err(decode)?;
    decoder.decode().map_err(decode)
}

fn size(terminal: &Terminal<'_, '_>) -> Size {
    Size { cols: terminal.cols().unwrap_or(0), rows: terminal.rows().unwrap_or(0) }
}

fn visible_row(terminal: &Terminal<'_, '_>, cols: u16, y: u16) -> RowCells {
    cells::row(terminal, cols, |x| Point::Active(Coordinate { x, y: u32::from(y) }))
}

#[cfg(test)]
mod tests;
