//! The [`Screen`] trait over the vt100 crate.

use std::fmt;
use std::panic::{self, AssertUnwindSafe};

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
/// - A dimension of 0 becomes 1. vt100 0.16.2 panics when a line wraps on a grid of
///   one row, so a one-row screen runs on two rows of vt100 and shows the row the
///   cursor is on: text that arrives line by line looks as it would on one row, while
///   moving the cursor up or down, a no-op on a real single row, switches rows.
/// - vt100 0.16.2 also panics on a few rare inputs (a wide character on a grid of one
///   column, or printing over a wide character that a narrower resize cut at the
///   right edge). The screen catches the panic, logs it and starts over blank with the
///   same size, title and working directory, so a vt100 bug cannot end the screen
///   actor; the rest of that chunk is lost.
pub struct Vt100Screen {
    parser: vt100::Parser<Recorder>,
    /// The grid the owner asked for, at least 1 by 1. vt100's own grid may be taller;
    /// see [`grid_size`].
    size: Size,
    /// The scrollback capacity, kept to rebuild the parser after a vt100 panic.
    scrollback_rows: usize,
}

impl Vt100Screen {
    /// A blank screen of `size` with [`DEFAULT_SCROLLBACK_ROWS`] rows of scrollback.
    pub fn new(size: Size) -> Self {
        Self::with_scrollback(size, DEFAULT_SCROLLBACK_ROWS)
    }

    /// A blank screen of `size` that keeps up to `scrollback_rows` rows that scrolled
    /// off the top. With 0 it keeps none.
    pub fn with_scrollback(size: Size, scrollback_rows: usize) -> Self {
        let size = visible_size(size);
        Vt100Screen {
            parser: parser(size, scrollback_rows, Recorder::default()),
            size,
            scrollback_rows,
        }
    }

    /// The vt100 row shown as the top visible row. It is 0 unless vt100 holds more
    /// rows than the owner asked for (one row on a grid of two); then the view is the
    /// rows that end at the cursor's row.
    fn top(&self) -> u16 {
        let screen = self.parser.screen();
        let (rows, _) = screen.size();
        let (cursor_row, _) = screen.cursor_position();
        cursor_row
            .saturating_sub(self.size.rows.saturating_sub(1))
            .min(rows.saturating_sub(self.size.rows))
    }

    /// Up to `limit` rows of scrollback, oldest first. On a one-row screen the vt100
    /// rows above the view are the newest of them.
    fn scrollback(&mut self, limit: usize) -> Vec<RowCells> {
        let top = self.top();
        let above = top.min(u16::try_from(limit).unwrap_or(u16::MAX));
        let mut scrollback = self.vt100_scrollback(limit - usize::from(above));
        let screen = self.parser.screen();
        scrollback.extend((top - above..top).map(|index| cells::row(screen, index)));
        scrollback
    }

    /// Up to `limit` rows of vt100's own scrollback, oldest first.
    ///
    /// vt100 shows scrollback only through a scrolled view whose top rows are
    /// scrollback, so the view moves down from the oldest wanted row a screen at a
    /// time and ends back at the bottom, where every other method expects it.
    fn vt100_scrollback(&mut self, limit: usize) -> Vec<RowCells> {
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

    /// Replaces a parser that panicked with a blank one; see [`Screen::feed`].
    fn start_over(&mut self) {
        tracing::warn!(
            cols = self.size.cols,
            rows = self.size.rows,
            "vt100 panicked while parsing; the screen starts over blank"
        );
        let recorder = std::mem::take(self.parser.callbacks_mut());
        self.parser = parser(self.size, self.scrollback_rows, recorder);
    }
}

/// Builds [`Vt100Screen`]s for `ScreenActor::spawn` and the conformance suite. The
/// closure runs on the screen's own thread and makes a screen of `size` with the
/// default scrollback.
pub fn factory(size: Size) -> impl FnOnce() -> Vt100Screen + Send + 'static {
    move || Vt100Screen::new(size)
}

/// The size a screen shows. vt100 subtracts 1 from both dimensions whenever it builds
/// or resizes a grid, so neither may be 0.
fn visible_size(size: Size) -> Size {
    Size { cols: size.cols.max(1), rows: size.rows.max(1) }
}

/// vt100's own grid as (rows, cols) for a visible size: a second row under a
/// one-row screen, because vt100 0.16.2 underflows when a line wraps on a single row.
fn grid_size(size: Size) -> (u16, u16) {
    (size.rows.max(2), size.cols)
}

fn parser(size: Size, scrollback_rows: usize, recorder: Recorder) -> vt100::Parser<Recorder> {
    let (rows, cols) = grid_size(size);
    vt100::Parser::new_with_callbacks(rows, cols, scrollback_rows, recorder)
}

impl Screen for Vt100Screen {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        let parser = &mut self.parser;
        // NOTE: a panic inside vt100 would otherwise end the screen actor, and the
        // shell would lose its screen for good. AssertUnwindSafe holds because a
        // parser that panicked is dropped unread; the recorder kept from it only
        // appends whole values in callbacks that cannot panic.
        if panic::catch_unwind(AssertUnwindSafe(|| parser.process(bytes))).is_err() {
            self.start_over();
        }
        self.parser.callbacks_mut().drain_into(sink);
    }

    fn resize(&mut self, cols: u16, rows: u16, _sink: &mut dyn ScreenSink) {
        // vt100 has no in-band resize report (mode 2048), so the program hears nothing
        // and neither does the sink.
        self.size = visible_size(Size { cols, rows });
        let (rows, cols) = grid_size(self.size);
        self.parser.screen_mut().set_size(rows, cols);
    }

    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot {
        let scrollback = self.scrollback(scrollback_rows);
        let top = self.top();
        let screen = self.parser.screen();
        ScreenSnapshot {
            size: self.size,
            cursor: self.cursor(),
            rows: (top..top + self.size.rows).map(|index| cells::row(screen, index)).collect(),
            scrollback,
            title: self.title().map(str::to_owned),
            alternate_screen: screen.alternate_screen(),
        }
    }

    fn row(&self, index: usize) -> RowCells {
        match u16::try_from(index) {
            Ok(index) if index < self.size.rows => {
                cells::row(self.parser.screen(), self.top() + index)
            }
            _ => RowCells::default(),
        }
    }

    fn cursor(&self) -> Cursor {
        let screen = self.parser.screen();
        let (row, col) = screen.cursor_position();
        // After a character in the last column vt100 parks the cursor one column past
        // the edge until the next character wraps; the wire cursor is always inside
        // the grid.
        Cursor {
            row: row.saturating_sub(self.top()).min(self.size.rows - 1),
            col: col.min(self.size.cols - 1),
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
        f.debug_struct("Vt100Screen")
            .field("size", &self.size)
            .field("cursor", &self.cursor())
            .field("alternate_screen", &self.parser.screen().alternate_screen())
            .field("title", &self.title())
            .field("pwd", &self.pwd())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
