//! The trait every terminal backend implements.

use std::time::Duration;

use efr_protocol::{ScreenSnapshot, Seq};

use crate::snapshot::normalize;
use crate::{ScreenSink, ScreenText, ShellMark};

/// A terminal emulator that turns PTY output into a grid of cells.
///
/// Implemented by [`Vt100Screen`](crate::vt100::Vt100Screen) and `efr-screen-ghostty`.
/// The trait deliberately has no `Send` bound: libghostty-vt types are neither `Send`
/// nor `Sync`, so a screen is built on its actor thread by a `Send` factory and never
/// leaves it (see
/// [`ScreenActor::spawn`](crate::ScreenActor::spawn)). Other crates never hold a
/// `Screen`; they talk to it through a [`ScreenHandle`](crate::ScreenHandle).
///
/// Shell marks (OSC 133 and OSC 7) never come from a `Screen`. The actor scans every
/// chunk once with a [`ShellMarkScanner`](crate::ShellMarkScanner) and gives the
/// marks to [`feed_marked`](Screen::feed_marked), so both backends report identical
/// marks with recording offsets, and a backend that parses the sequences itself can
/// check its own view against them without a second scan.
pub trait Screen {
    /// Processes PTY output. Answers to terminal queries (DA, DSR, DECRQM, OSC 10 and
    /// 11), bells and title changes go to `sink` while the bytes are processed.
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink);

    /// [`feed`](Screen::feed) with what the actor knows of the bytes: the recording
    /// offset `base` of their first byte and the shell marks that its scanner found
    /// in them, which end inside them, in stream order. The actor calls this, never
    /// `feed`. The default ignores the marks.
    fn feed_marked(
        &mut self,
        bytes: &[u8],
        base: Seq,
        marks: &[ShellMark],
        sink: &mut dyn ScreenSink,
    ) {
        let _ = (base, marks);
        self.feed(bytes, sink);
    }

    /// Changes the size of the visible grid. A backend that reports size changes to
    /// the program (in-band resize, mode 2048) writes the report to `sink`.
    fn resize(&mut self, cols: u16, rows: u16, sink: &mut dyn ScreenSink);

    /// The visible grid, the cursor, the title and up to `scrollback_rows` rows of
    /// scrollback, newest last. The actor normalises the result before it leaves the
    /// thread, so a backend may return untrimmed rows.
    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot;

    /// The text of the scrollback and the visible rows, or `None` when the scrollback
    /// holds `scrollback_limit` rows or more. A backend that keeps no more than that
    /// many may have lost the oldest rows then, so the caller reads the bytes in
    /// smaller pieces instead.
    ///
    /// The default asks [`scrollback_rows`](Screen::scrollback_rows) first and reads
    /// no rows when the scrollback is full. Otherwise it reads a
    /// [`snapshot`](Screen::snapshot) with up to `scrollback_limit` rows and gives
    /// [`ScreenText::of`] its normalised form. A backend that can format its text
    /// more cheaply overrides it, and must give the same text, except at these rare
    /// wrap edges, where the text of the cells is mostly the worse one: a soft-wrapped
    /// row that an erase emptied, a row that only a tab moved over before a wide
    /// character wrapped, the empty last cell that a wide character left (with or
    /// without a colour), and a space that the program printed in the last column
    /// before a wide character. The libghostty-vt backend tests each edge
    /// (`the_formatter_and_the_cells_differ_only_at_these_edges`).
    fn text(&mut self, scrollback_limit: usize) -> Option<ScreenText> {
        cells_text(self, scrollback_limit)
    }

    /// The rows of scrollback that a [`snapshot`](Screen::snapshot) with all of it
    /// gives. A backend counts them without reading them, so that
    /// [`text`](Screen::text) reads no row of a full scrollback.
    fn scrollback_rows(&mut self) -> usize;

    /// True while the alternate screen is up, as a full-screen program sets it. The
    /// default reads a [`snapshot`](Screen::snapshot) without scrollback; a backend
    /// that can tell more cheaply overrides it.
    fn alternate_screen(&mut self) -> bool {
        self.snapshot(0).alternate_screen
    }

    /// How long the screen must get no command before the actor calls
    /// [`idle`](Screen::idle). The actor reads it once, when the screen starts.
    /// `None`, the default, means never: the actor then only waits for commands.
    fn idle_delay(&self) -> Option<Duration> {
        None
    }

    /// One short step of work for a quiet moment, such as the compression of old
    /// scrollback. The actor calls it when no command came for
    /// [`idle_delay`](Screen::idle_delay) after the last command. It returns how long
    /// the actor waits for a command before the next step, or `None` when no work is
    /// left until the next command. A command that comes first is served first. The
    /// default does nothing.
    fn idle(&mut self) -> Option<Duration> {
        None
    }
}

/// The text of `screen` from its cells: the default of [`Screen::text`], which a
/// backend that formats its text itself keeps for when that fails. It asks
/// [`Screen::scrollback_rows`] first and reads no rows when the scrollback holds
/// `scrollback_limit` rows or more.
pub fn cells_text<S: Screen + ?Sized>(
    screen: &mut S,
    scrollback_limit: usize,
) -> Option<ScreenText> {
    if screen.scrollback_rows() >= scrollback_limit {
        return None;
    }
    let snapshot = screen.snapshot(scrollback_limit);
    Some(ScreenText::of(&normalize(snapshot, scrollback_limit)))
}
