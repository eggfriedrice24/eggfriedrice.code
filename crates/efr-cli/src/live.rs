//! Redrawing the live zone in place: the part of a reply that is still arriving.
//!
//! Committed output is written once and scrolls away like any other output. The live
//! zone sits below it and is replaced on every update: a carriage return, the cursor up
//! over the rows the old live zone takes, erase to the end of the screen, then the new
//! committed output and the new live zone, all inside synchronized output (mode 2026)
//! so the terminal shows the result without flicker.
//!
//! Moving up needs the old live zone's height in terminal rows, which depends on the
//! width: a line wider than the terminal wraps onto more rows, and a terminal that
//! reflows on resize wraps the same lines differently afterwards. So the height is
//! measured again at the current width when the width changed since it was drawn.
//! Every live line ends with a newline, so after a redraw the cursor sits at the start
//! of the row below the live zone.
//!
//! The live zone is kept smaller than the screen: rows that scrolled off the top can
//! no longer be reached by moving the cursor up, so they could never be erased. When
//! it would be taller, only its last lines are shown.
//!
//! The rows are counted as the terminal counts columns: by code point, or by grapheme
//! cluster in Ghostty ([`WidthMethod`]). A ZWJ emoji, a flag or a variation selector
//! counted the other way would move the cursor up one row too many or too few.
//!
//! The last line of the live zone can be a status row, which changes on every tick of a
//! running turn. When only that row changed, the redraw replaces that one row: a
//! carriage return, the cursor up one row, erase the line, the new row. The rest of the
//! live zone stays on the screen as it is.

use std::fmt::Write as _;

use efr_render::{WidthMethod, display_width};

use crate::terminal::Size;

/// Starts synchronized output: the terminal holds the screen until the end mark.
const BEGIN_SYNC: &str = "\x1b[?2026h";
/// Ends synchronized output.
const END_SYNC: &str = "\x1b[?2026l";
/// The width when the terminal does not report one, the same fallback as
/// `efr_render::RenderOptions`.
const FALLBACK_WIDTH: u16 = 80;

/// A live zone's height as the renderer measured it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Measured {
    /// The rows it takes.
    pub(crate) rows: usize,
    /// The width the rows were counted at.
    pub(crate) width: u16,
}

/// The live zone as it stands on the screen.
#[derive(Debug, Default)]
pub(crate) struct LiveZone {
    /// How the terminal counts the width of text.
    method: WidthMethod,
    /// The text on the screen now, after clipping.
    shown: String,
    /// The status row at the end of `shown`, with its newline; empty without one.
    status: String,
    /// The rows it took when it was written.
    rows: usize,
    /// The width it was written at.
    width: u16,
}

impl LiveZone {
    /// An empty live zone on a terminal that counts widths by `method`.
    pub(crate) fn new(method: WidthMethod) -> LiveZone {
        LiveZone { method, ..LiveZone::default() }
    }

    /// The bytes that erase the live zone, write `committed` once, and show `live` in
    /// its place. `measured` is the renderer's count of the rows `live` takes, used
    /// when it was counted at the current width. Empty when nothing would change.
    #[cfg(test)]
    pub(crate) fn redraw(
        &mut self,
        committed: &str,
        live: &str,
        measured: Option<Measured>,
        size: Size,
    ) -> String {
        self.draw(committed, live, measured, "", size)
    }

    /// The bytes that erase the live zone, write `committed` once, and show `body` and
    /// then the status row `status` (one line with its newline, or empty) in its place.
    /// `measured` is the renderer's count of the rows `body` takes, used when it was
    /// counted at the current width. When only the status row changed, only that row is
    /// written again. Empty when nothing would change.
    pub(crate) fn draw(
        &mut self,
        committed: &str,
        body: &str,
        measured: Option<Measured>,
        status: &str,
        size: Size,
    ) -> String {
        let width = effective_width(size);
        let method = self.method;
        let rows_of = |text: &str| rows_of(text, width, method);
        let live = format!("{body}{status}");
        let (shown, rows) = match measured {
            Some(measured)
                if measured.width == width && fits(measured.rows + rows_of(status), size) =>
            {
                (live.as_str(), measured.rows + rows_of(status))
            }
            _ => clip(&live, width, max_rows(size), method),
        };
        // An empty live zone looks the same at any width.
        if committed.is_empty() && shown == self.shown && (width == self.width || shown.is_empty())
        {
            return String::new();
        }
        // The status row stays one row when the rest stays as it is: only it changes.
        let status = if status.is_empty() || !shown.ends_with(status) { "" } else { status };
        let same_rest = shown.strip_suffix(status) == self.shown.strip_suffix(&*self.status);
        let mut out = String::from(BEGIN_SYNC);
        if committed.is_empty()
            && width == self.width
            && rows == self.rows
            && same_rest
            && rows_of(status) == 1
            && rows_of(&self.status) == 1
        {
            out.push_str("\r\x1b[1A\x1b[2K");
            out.push_str(status);
        } else {
            let old_rows = if width == self.width { self.rows } else { rows_of(&self.shown) };
            if old_rows > 0 {
                let _ = write!(out, "\r\x1b[{old_rows}A\x1b[J");
            }
            out.push_str(committed);
            out.push_str(shown);
        }
        out.push_str(END_SYNC);
        shown.clone_into(&mut self.shown);
        status.clone_into(&mut self.status);
        self.rows = rows;
        self.width = width;
        out
    }
}

/// The width that rendering and row counting use for a terminal of `size`.
pub(crate) fn effective_width(size: Size) -> u16 {
    if size.cols == 0 { FALLBACK_WIDTH } else { size.cols }
}

/// The most rows the live zone may take: one less than the screen, so it never fills
/// it. `None` when the height is unknown.
fn max_rows(size: Size) -> Option<usize> {
    (size.rows > 0).then(|| usize::from(size.rows) - 1)
}

fn fits(rows: usize, size: Size) -> bool {
    max_rows(size).is_none_or(|max| rows <= max)
}

/// The last whole lines of `live` that fit in `max_rows` rows at `width`, and the rows
/// they take. Whole lines only: every live line opens and closes its own styles.
fn clip(live: &str, width: u16, max_rows: Option<usize>, method: WidthMethod) -> (&str, usize) {
    let Some(max_rows) = max_rows else {
        return (live, rows_of(live, width, method));
    };
    let mut rows = 0;
    let mut start = live.len();
    for line in live.split_inclusive('\n').rev() {
        let line_rows = line_rows(line, width, method);
        if rows + line_rows > max_rows {
            break;
        }
        rows += line_rows;
        start -= line.len();
    }
    (&live[start..], rows)
}

/// The rows `text` takes on a terminal `width` columns wide that counts widths by
/// `method`: each line at least one, and one more for every time it wraps.
pub(crate) fn rows_of(text: &str, width: u16, method: WidthMethod) -> usize {
    text.split_inclusive('\n').map(|line| line_rows(line, width, method)).sum()
}

fn line_rows(line: &str, width: u16, method: WidthMethod) -> usize {
    let line = line.strip_suffix('\n').unwrap_or(line);
    display_width(line, method).div_ceil(usize::from(width.max(1))).max(1)
}

#[cfg(test)]
mod tests;
