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

use std::fmt::Write as _;

use unicode_width::UnicodeWidthChar as _;

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
    /// The text on the screen now, after clipping.
    shown: String,
    /// The rows it took when it was written.
    rows: usize,
    /// The width it was written at.
    width: u16,
}

impl LiveZone {
    /// The bytes that erase the live zone, write `committed` once, and show `live` in
    /// its place. `measured` is the renderer's count of the rows `live` takes, used
    /// when it was counted at the current width. Empty when nothing would change.
    pub(crate) fn redraw(
        &mut self,
        committed: &str,
        live: &str,
        measured: Option<Measured>,
        size: Size,
    ) -> String {
        let width = effective_width(size);
        let (shown, rows) = match measured {
            Some(measured) if measured.width == width && fits(measured.rows, size) => {
                (live, measured.rows)
            }
            _ => clip(live, width, max_rows(size)),
        };
        if committed.is_empty() && shown == self.shown && width == self.width {
            return String::new();
        }
        let old_rows = if width == self.width { self.rows } else { rows_of(&self.shown, width) };
        let mut out = String::from(BEGIN_SYNC);
        if old_rows > 0 {
            let _ = write!(out, "\r\x1b[{old_rows}A\x1b[J");
        }
        out.push_str(committed);
        out.push_str(shown);
        out.push_str(END_SYNC);
        self.shown = shown.to_owned();
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
fn clip(live: &str, width: u16, max_rows: Option<usize>) -> (&str, usize) {
    let Some(max_rows) = max_rows else {
        return (live, rows_of(live, width));
    };
    let mut rows = 0;
    let mut start = live.len();
    for line in live.split_inclusive('\n').rev() {
        let line_rows = line_rows(line, width);
        if rows + line_rows > max_rows {
            break;
        }
        rows += line_rows;
        start -= line.len();
    }
    (&live[start..], rows)
}

/// The rows `text` takes on a terminal `width` columns wide: each line at least one,
/// and one more for every time it wraps.
pub(crate) fn rows_of(text: &str, width: u16) -> usize {
    text.split_inclusive('\n').map(|line| line_rows(line, width)).sum()
}

fn line_rows(line: &str, width: u16) -> usize {
    let line = line.strip_suffix('\n').unwrap_or(line);
    display_width(line).div_ceil(usize::from(width.max(1))).max(1)
}

/// The columns that painted text takes, without its CSI sequences (colours) and OSC
/// sequences (hyperlinks).
pub(crate) fn display_width(painted: &str) -> usize {
    let mut width = 0;
    let mut chars = painted.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            width += c.width().unwrap_or(0);
            continue;
        }
        match chars.next() {
            // A CSI sequence ends with its final byte, 0x40 to 0x7e.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // An OSC sequence ends with BEL or with ST (ESC \).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    width
}

#[cfg(test)]
mod tests;
