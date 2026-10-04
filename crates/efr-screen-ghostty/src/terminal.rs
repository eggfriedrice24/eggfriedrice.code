//! [`GhosttyScreen`]: the `Screen` trait over a libghostty-vt terminal.

use std::rc::Rc;

use efr_screen::{Cursor, RowCells, Screen, ScreenSink, ScreenSnapshot, Size};
use libghostty_vt::Terminal;
use libghostty_vt::style::RgbColor;

use crate::GhosttyError;
use crate::effects::Effects;
use crate::snapshot;

/// The pixel size of one cell, which libghostty-vt puts into in-band resize reports
/// (mode 2048). A hidden terminal has no pixels, but a program that divides by the
/// cell size must not see zero, so a nominal 8x16 cell is reported.
const CELL_WIDTH_PX: u32 = 8;
const CELL_HEIGHT_PX: u32 = 16;

/// Settings of a [`GhosttyScreen`] that its owner may change.
///
/// Build it from [`GhosttyConfig::default`] and change the fields that differ.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GhosttyConfig {
    /// Lines of scrollback kept above the visible grid. libghostty-vt prunes whole
    /// pages, so it keeps somewhat more. The default is 5000.
    pub scrollback_lines: usize,
    /// The default foreground colour as red, green and blue: what an OSC 10 query is
    /// answered with. `None` leaves OSC 10 unanswered. The default is ghostty's own,
    /// `#ffffff`.
    pub foreground: Option<[u8; 3]>,
    /// The default background colour: what an OSC 11 query is answered with, and so
    /// what tells a program whether the terminal is dark. `None` leaves OSC 11
    /// unanswered. The default is ghostty's own, `#282c34`.
    pub background: Option<[u8; 3]>,
}

impl Default for GhosttyConfig {
    fn default() -> Self {
        GhosttyConfig {
            scrollback_lines: 5000,
            foreground: Some([0xff, 0xff, 0xff]),
            background: Some([0x28, 0x2c, 0x34]),
        }
    }
}

/// A screen backed by a libghostty-vt terminal.
///
/// Like every libghostty-vt type it is neither `Send` nor `Sync`, so it is built on
/// its actor thread by a `Send` factory ([`factory`], [`factory_with`]) and never
/// leaves it. The terminal is created with `Terminal::new` and its callbacks own
/// everything they touch, so the screen borrows nothing and is `'static`.
///
/// Answers to terminal queries, bells and title changes are collected while
/// libghostty-vt runs and handed to the [`ScreenSink`] after each feed and resize.
/// A feed never fails: malformed input only changes the screen.
#[derive(Debug)]
pub struct GhosttyScreen {
    terminal: Terminal<'static, 'static>,
    effects: Rc<Effects>,
}

impl GhosttyScreen {
    /// A blank screen of `size`. A zero width or height is raised to one, because a
    /// terminal needs at least one cell.
    ///
    /// # Errors
    ///
    /// [`GhosttyError::Create`] when libghostty-vt cannot allocate the terminal, and
    /// [`GhosttyError::Configure`] when it refuses a setting of `config`.
    pub fn new(size: Size, config: &GhosttyConfig) -> Result<Self, GhosttyError> {
        let (cols, rows) = grid(size.cols, size.rows);
        let terminal = Terminal::new(cols, rows).map_err(|source| GhosttyError::Create {
            cols,
            rows,
            source,
        })?;
        Self::configure(terminal, config)
    }

    fn configure(
        mut terminal: Terminal<'static, 'static>,
        config: &GhosttyConfig,
    ) -> Result<Self, GhosttyError> {
        let configure = |source| GhosttyError::Configure { source };
        let cols = terminal.cols().map_err(configure)?;
        let rows = terminal.rows().map_err(configure)?;
        terminal
            .set_scrollback_max_lines(Some(config.scrollback_lines))
            .and_then(|terminal| terminal.set_scrollback_max_bytes(None))
            .and_then(|terminal| terminal.set_default_fg_color(config.foreground.map(rgb)))
            .and_then(|terminal| terminal.set_default_bg_color(config.background.map(rgb)))
            .map_err(configure)?;
        // Terminal::new knows no cell size; a resize to the same grid sets it.
        terminal.resize(cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX).map_err(configure)?;
        let effects = Effects::install(&mut terminal).map_err(configure)?;
        Ok(GhosttyScreen { terminal, effects })
    }
}

impl Screen for GhosttyScreen {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        self.terminal.vt_write(bytes);
        self.effects.drain(sink);
    }

    fn resize(&mut self, cols: u16, rows: u16, sink: &mut dyn ScreenSink) {
        let (cols, rows) = grid(cols, rows);
        if let Err(error) = self.terminal.resize(cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX) {
            tracing::warn!(cols, rows, %error, "the ghostty screen kept its size after a failed resize");
        }
        self.effects.drain(sink);
    }

    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot {
        snapshot::capture(&self.terminal, scrollback_rows)
    }

    fn row(&self, index: usize) -> RowCells {
        snapshot::active_row(&self.terminal, index)
    }

    fn cursor(&self) -> Cursor {
        snapshot::cursor(&self.terminal)
    }

    fn title(&self) -> Option<&str> {
        snapshot::title(&self.terminal)
    }

    /// The working directory as libghostty-vt stores it: the raw OSC 7 URL
    /// (`kitty-shell-cwd://host/path` or `file://host/path`), not a decoded path.
    fn pwd(&self) -> Option<&str> {
        self.terminal.pwd().ok().filter(|pwd| !pwd.is_empty())
    }
}

/// A factory for [`ScreenActor::spawn`](efr_screen::ScreenActor::spawn) that builds a
/// blank ghostty screen of `size` with the default [`GhosttyConfig`].
///
/// The factory runs on the actor thread. If libghostty-vt cannot create the terminal
/// (it is out of memory), the factory logs the error and panics; the actor turns a
/// backend panic into [`ScreenError::Closed`](efr_screen::ScreenError::Closed) for
/// every handle, which is how the owner learns the screen never started.
pub fn factory(size: Size) -> impl FnOnce() -> GhosttyScreen + Send + 'static {
    factory_with(size, GhosttyConfig::default())
}

/// [`factory`] with a [`GhosttyConfig`] of the owner's choosing.
pub fn factory_with(
    size: Size,
    config: GhosttyConfig,
) -> impl FnOnce() -> GhosttyScreen + Send + 'static {
    move || match GhosttyScreen::new(size, &config) {
        Ok(screen) => screen,
        Err(err) => not_started(&err),
    }
}

/// Ends the actor thread when a factory cannot build its screen.
#[expect(
    clippy::panic,
    reason = "a factory cannot return an error; the actor turns this panic into ScreenError::Closed"
)]
fn not_started(err: &GhosttyError) -> ! {
    tracing::error!(
        error = err as &(dyn std::error::Error + 'static),
        "the ghostty screen could not start"
    );
    panic!("the ghostty screen could not start: {err}");
}

/// The grid libghostty-vt accepts: at least one column and one row.
fn grid(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(1), rows.max(1))
}

fn rgb([r, g, b]: [u8; 3]) -> RgbColor {
    RgbColor { r, g, b }
}

#[cfg(test)]
mod tests;
