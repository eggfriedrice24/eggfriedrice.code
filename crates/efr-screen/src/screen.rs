//! The trait every terminal backend implements.

use efr_protocol::{Cursor, RowCells, ScreenSnapshot};

use crate::ScreenSink;

/// A terminal emulator that turns PTY output into a grid of cells.
///
/// Implemented by `efr-screen-vt100` and `efr-screen-ghostty`. The trait deliberately
/// has no `Send` bound: libghostty-vt types are neither `Send` nor `Sync`, so a screen
/// is built on its actor thread by a `Send` factory and never leaves it (see
/// [`ScreenActor::spawn`](crate::ScreenActor::spawn)). Other crates never hold a
/// `Screen`; they talk to it through a [`ScreenHandle`](crate::ScreenHandle).
///
/// Shell marks (OSC 133 and OSC 7) are not a `Screen` method. The actor scans every
/// chunk with a [`ShellMarkScanner`](crate::ShellMarkScanner) before it calls
/// [`feed`](Screen::feed), so both backends report identical marks with recording
/// offsets.
pub trait Screen {
    /// Processes PTY output. Answers to terminal queries (DA, DSR, DECRQM, OSC 10 and
    /// 11), bells and title changes go to `sink` while the bytes are processed.
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink);

    /// Changes the size of the visible grid. A backend that reports size changes to
    /// the program (in-band resize, mode 2048) writes the report to `sink`.
    fn resize(&mut self, cols: u16, rows: u16, sink: &mut dyn ScreenSink);

    /// The visible grid, the cursor, the title and up to `scrollback_rows` rows of
    /// scrollback, newest last. The actor normalises the result before it leaves the
    /// thread, so a backend may return untrimmed rows.
    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot;

    /// One row of the visible grid, counted from 0 at the top. A row outside the grid
    /// is empty.
    fn row(&self, index: usize) -> RowCells;

    /// The cursor position in the visible grid.
    fn cursor(&self) -> Cursor;

    /// The window title that the program set, if any.
    fn title(&self) -> Option<&str>;

    /// The backend's own view of the OSC 7 working directory. It is a cross-check
    /// against the scanner's marks only; the marks are the source of truth.
    fn pwd(&self) -> Option<&str>;
}
