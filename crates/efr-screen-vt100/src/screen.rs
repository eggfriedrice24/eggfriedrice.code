//! The [`Screen`] trait over the vt100 crate.

use std::fmt;

use efr_screen::{Cursor, RowCells, Screen, ScreenSink, ScreenSnapshot, Size};

use crate::cells;
use crate::recorder::Recorder;

/// The rows of scrollback [`Vt100Screen::new`] keeps. A vt100 cell takes 32 bytes, so
/// a full scrollback at 200 columns costs about 6 MiB per screen. Attach snapshots
/// are what reads it; the recording, not the screen, holds a command's whole output.
pub const DEFAULT_SCROLLBACK_ROWS: usize = 1_000;

/// A terminal screen backed by the vt100 crate (0.16.2).
///
/// It needs no Zig, so it is the backend of every test and of builds without
/// libghostty-vt. Build it on its screen thread with [`factory`] and
/// `ScreenActor::spawn`, like every backend.
///
/// - It answers no terminal queries. vt100 parses DA, DSR, DECRQM and OSC 10 and 11
///   and drops them, so nothing reaches [`ScreenSink::pty_reply`]; the conformance
///   fixtures that check replies list `vt100` in their `differs`.
/// - It reports bells (BEL) and titles (OSC 0 and 2) to the sink in the order they
///   happened, after the bytes are processed.
/// - [`title`](Screen::title) and [`pwd`](Screen::pwd) follow libghostty-vt: an empty
///   value clears them, a title is cut at 1024 bytes and the OSC 7 URL at 4096, and
///   `pwd` is the raw URL as the program sent it.
/// - vt100 cannot hold a grid without rows or columns, so a dimension of 0 becomes 1.
pub struct Vt100Screen {
    parser: vt100::Parser<Recorder>,
}

impl Vt100Screen {
    /// A blank screen of `size` with [`DEFAULT_SCROLLBACK_ROWS`] rows of scrollback.
    pub fn new(size: Size) -> Self {
        Self::with_scrollback(size, DEFAULT_SCROLLBACK_ROWS)
    }

    /// A blank screen of `size` that keeps up to `scrollback_rows` rows that scrolled
    /// off the top. With 0 it keeps none.
    pub fn with_scrollback(size: Size, scrollback_rows: usize) -> Self {
        let parser = vt100::Parser::new_with_callbacks(
            dimension(size.rows),
            dimension(size.cols),
            scrollback_rows,
            Recorder::default(),
        );
        Vt100Screen { parser }
    }

    /// Up to `limit` rows of scrollback, oldest first.
    ///
    /// vt100 shows scrollback only through a scrolled view whose top rows are
    /// scrollback, so the view moves down from the oldest wanted row a screen at a
    /// time and ends back at the bottom, where every other method expects it.
    fn scrollback(&mut self, limit: usize) -> Vec<RowCells> {
        let screen = self.parser.screen_mut();
        // vt100 clamps the offset to the rows it holds.
        screen.set_scrollback(limit);
        let mut offset = screen.scrollback();
        let (rows, _) = screen.size();
        let mut scrollback = Vec::with_capacity(offset);
        while offset > 0 {
            screen.set_scrollback(offset);
            let take = u16::try_from(offset).map_or(rows, |offset| offset.min(rows));
            scrollback.extend((0..take).map(|index| cells::row(screen, index)));
            offset -= usize::from(take);
        }
        screen.set_scrollback(0);
        scrollback
    }
}

/// Builds [`Vt100Screen`]s for `ScreenActor::spawn` and the conformance suite. The
/// closure runs on the screen's own thread and makes a screen of `size` with the
/// default scrollback.
pub fn factory(size: Size) -> impl FnOnce() -> Vt100Screen + Send + 'static {
    move || Vt100Screen::new(size)
}

/// vt100 subtracts 1 from both dimensions whenever it builds or resizes a grid.
fn dimension(cells: u16) -> u16 {
    cells.max(1)
}

impl Screen for Vt100Screen {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        self.parser.process(bytes);
        self.parser.callbacks_mut().drain_into(sink);
    }

    fn resize(&mut self, cols: u16, rows: u16, _sink: &mut dyn ScreenSink) {
        // vt100 has no in-band resize report (mode 2048), so the program hears nothing
        // and neither does the sink.
        self.parser.screen_mut().set_size(dimension(rows), dimension(cols));
    }

    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot {
        let scrollback = self.scrollback(scrollback_rows);
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        ScreenSnapshot {
            size: Size { cols, rows },
            cursor: self.cursor(),
            rows: (0..rows).map(|index| cells::row(screen, index)).collect(),
            scrollback,
            title: self.title().map(str::to_owned),
            alternate_screen: screen.alternate_screen(),
        }
    }

    fn row(&self, index: usize) -> RowCells {
        let screen = self.parser.screen();
        let (rows, _) = screen.size();
        match u16::try_from(index) {
            Ok(index) if index < rows => cells::row(screen, index),
            _ => RowCells::default(),
        }
    }

    fn cursor(&self) -> Cursor {
        let screen = self.parser.screen();
        let (row, col) = screen.cursor_position();
        let (rows, cols) = screen.size();
        // After a character in the last column vt100 parks the cursor one column past
        // the edge until the next character wraps; the wire cursor is always inside
        // the grid.
        Cursor {
            row: row.min(rows.saturating_sub(1)),
            col: col.min(cols.saturating_sub(1)),
            hidden: screen.hide_cursor(),
        }
    }

    fn title(&self) -> Option<&str> {
        self.parser.callbacks().title()
    }

    fn pwd(&self) -> Option<&str> {
        self.parser.callbacks().pwd()
    }
}

// vt100::Parser has no Debug of its own.
impl fmt::Debug for Vt100Screen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (rows, cols) = self.parser.screen().size();
        f.debug_struct("Vt100Screen")
            .field("size", &Size { cols, rows })
            .field("cursor", &self.cursor())
            .field("alternate_screen", &self.parser.screen().alternate_screen())
            .field("title", &self.title())
            .field("pwd", &self.pwd())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
