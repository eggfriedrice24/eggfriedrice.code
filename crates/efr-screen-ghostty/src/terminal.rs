//! [`GhosttyScreen`]: the `Screen` trait over a libghostty-vt terminal.

use std::rc::Rc;
use std::time::Duration;

use efr_screen::{Cursor, RowCells, Screen, ScreenSink, ScreenSnapshot, Seq, ShellMark, Size};
use libghostty_vt::Terminal;
use libghostty_vt::color::Rgb;
use libghostty_vt::screen::Screen as ActiveScreen;
use libghostty_vt::terminal::{CompressionActivity, CompressionMode, CompressionResult};

use crate::GhosttyError;
use crate::effects::Effects;
use crate::semantic_prompt::CrossCheck;
use crate::{ffi, snapshot};

/// The pixel size of one cell, which libghostty-vt puts into in-band resize reports
/// (mode 2048). A hidden terminal has no pixels, but a program that divides by the
/// cell size must not see zero, so a nominal 8x16 cell is reported.
const CELL_WIDTH_PX: u32 = 8;
const CELL_HEIGHT_PX: u32 = 16;

/// The longest unfinished escape sequence kept for a GHOSTSNP snapshot. A snapshot
/// taken while the parser is inside a longer one (a large DCS image) fails until
/// the sequence ends; ordinary sequences are far shorter.
const CONTINUATION_LIMIT: usize = 1 << 20;

/// The most bytes that libghostty-vt keeps for one APC string. libghostty-vt reads
/// two APC protocols, Kitty graphics and the Glyph Protocol, and `configure` turns
/// both off, so efr needs no APC: the terminal reads a command to its end and then
/// ignores it. This limit only bounds what one long or endless APC can make the
/// terminal keep: 8 KiB, not the library default of 65 MiB. 8 KiB still holds any
/// one Kitty graphics command that obeys the protocol, which sends data in chunks
/// of at most 4096 bytes with short control keys. Raise it together with the image
/// store if the hidden terminal ever shows images.
const APC_LIMIT: usize = 8 * 1024;

/// How long a screen must get no command before it compresses its old scrollback.
/// Ghostty waits 250 ms after its renderer is quiet. A hidden terminal draws
/// nothing, so it waits longer and does not compress in the short gaps of a build.
const COMPRESS_AFTER: Duration = Duration::from_secs(1);

/// The wait between two steps of one compression pass, as in Ghostty.
const COMPRESS_STEP: Duration = Duration::from_millis(1);

/// Settings of a [`GhosttyScreen`] that its owner may change.
///
/// Build it from [`GhosttyConfig::default`] and change the fields that differ.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GhosttyConfig {
    /// Lines of scrollback kept above the visible grid. libghostty-vt prunes whole
    /// pages, so it keeps up to one page more or less. The default is 5000.
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
/// its actor thread by a `Send` factory ([`factory`], [`factory_with`],
/// [`restore_factory`]) and never leaves it. The terminal is created with
/// `Terminal::new` and its callbacks own everything they touch, so the screen borrows
/// nothing and is `'static`.
///
/// Answers to terminal queries, bells and title changes are collected while
/// libghostty-vt runs and handed to the [`ScreenSink`] after each feed and resize.
/// Every feed from the actor is also cross-checked against the shell marks of the
/// actor's scan: the OSC 133 steps that libghostty-vt reports, its working directory
/// and its per-row prompt state must agree with the marks, at the byte where each
/// OSC ends. A disagreement is logged as a warning; a feed never fails.
#[derive(Debug)]
pub struct GhosttyScreen {
    terminal: Terminal<'static, 'static>,
    effects: Rc<Effects>,
    cross_check: CrossCheck,
    /// True once a failure inside libghostty-vt was logged. The library flag stays
    /// set for the life of the terminal, so it is logged once.
    failure_logged: bool,
    /// The compression activity of the terminal when its last compression pass
    /// ended. While the terminal reports the same value, there is nothing new to
    /// compress.
    compressed: Option<CompressionActivity>,
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
        ffi::init();
        let (cols, rows) = grid(size.cols, size.rows);
        let terminal = Terminal::new(cols, rows).map_err(|source| GhosttyError::Create {
            cols,
            rows,
            source,
        })?;
        Self::configure(terminal, config)
    }

    /// A screen restored from a GHOSTSNP snapshot made by
    /// [`encode_snapshot`](Self::encode_snapshot), here or in another process: the
    /// grid, the scrollback, the cursor, the modes, the title and the working
    /// directory, and the parser state, so a sequence that was cut off by the
    /// snapshot completes with the next feed.
    ///
    /// # Errors
    ///
    /// [`GhosttyError::DecodeSnapshot`] when the bytes are not a complete, intact
    /// snapshot, and [`GhosttyError::Configure`] when libghostty-vt refuses a setting
    /// of `config`.
    pub fn restore(snapshot: &[u8], config: &GhosttyConfig) -> Result<Self, GhosttyError> {
        ffi::init();
        let terminal = snapshot::decode(snapshot)?;
        Self::configure(terminal, config)
    }

    /// The whole terminal state as a GHOSTSNP snapshot: an authenticated record
    /// stream that [`restore`](Self::restore) turns back into an identical screen.
    /// Unlike [`Screen::snapshot`], it carries everything libghostty-vt knows, so a
    /// client with its own libghostty-vt can attach without losing modes or styles.
    ///
    /// # Errors
    ///
    /// [`GhosttyError::EncodeSnapshot`] while the parser is inside an escape sequence
    /// longer than the continuation limit (1 MiB); it succeeds again once the
    /// sequence has ended.
    pub fn encode_snapshot(&mut self) -> Result<Vec<u8>, GhosttyError> {
        snapshot::encode(&mut self.terminal)
    }

    fn configure(
        mut terminal: Terminal<'static, 'static>,
        config: &GhosttyConfig,
    ) -> Result<Self, GhosttyError> {
        let configure = |source| GhosttyError::Configure { source };
        let cols = terminal.cols().map_err(configure)?;
        let rows = terminal.rows().map_err(configure)?;
        // NOTE: the byte limit is `usize::MAX`, not `None`: from libghostty-vt 1.0.0 a
        // `None` means the library's default of 10,000 bytes, which cuts the
        // scrollback to one page, long before the line limit.
        terminal
            .set_scrollback_max_lines(Some(config.scrollback_lines))
            .and_then(|terminal| terminal.set_scrollback_max_bytes(Some(usize::MAX)))
            .and_then(|terminal| terminal.set_continuation_max_bytes(CONTINUATION_LIMIT))
            // NOTE: a hidden terminal draws nothing, so it must not say that it can.
            // A store limit of 0 turns the Kitty graphics protocol off on both
            // screens, and then the terminal does not answer the `a=q` support query.
            // Without this, the library keeps up to 10 MB of images on each screen.
            .and_then(|terminal| terminal.set_kitty_image_storage_limit(0))
            .and_then(|terminal| terminal.set_glyph_protocol_enabled(false))
            .and_then(|terminal| terminal.set_apc_max_bytes(Some(APC_LIMIT)))
            .and_then(|terminal| terminal.set_default_fg_color(config.foreground.map(rgb)))
            .and_then(|terminal| terminal.set_default_bg_color(config.background.map(rgb)))
            .map_err(configure)?;
        // Terminal::new knows no cell size; a resize to the same grid sets it.
        terminal.resize(cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX).map_err(configure)?;
        let effects = Effects::install(&mut terminal).map_err(configure)?;
        let cross_check = CrossCheck::new(&terminal);
        Ok(GhosttyScreen {
            terminal,
            effects,
            cross_check,
            failure_logged: false,
            compressed: None,
        })
    }

    /// Logs once when libghostty-vt failed inside a feed in a way that it did not
    /// handle, so the screen can miss a change that the program made. Input that the
    /// library refuses on purpose does not set the flag.
    fn log_failure(&mut self) {
        if !self.failure_logged && self.terminal.vt_processing_error().unwrap_or(false) {
            self.failure_logged = true;
            tracing::warn!(
                "libghostty-vt failed while it read the output of the program; the screen \
                 can miss a change from now on, and the log target {} can name the cause",
                ffi::TARGET
            );
        }
    }
}

impl Screen for GhosttyScreen {
    /// Writes the bytes without the cross-check, because there are no marks to
    /// check. The actor calls [`feed_marked`](Screen::feed_marked).
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        self.cross_check.write_unchecked(&mut self.terminal, &self.effects, bytes);
        self.log_failure();
        self.effects.drain(sink);
    }

    /// Writes the bytes and checks libghostty-vt's own reading of OSC 133 and OSC 7
    /// against the marks of the actor's scan.
    fn feed_marked(
        &mut self,
        bytes: &[u8],
        base: Seq,
        marks: &[ShellMark],
        sink: &mut dyn ScreenSink,
    ) {
        let check = &mut self.cross_check;
        for disagreement in check.write(&mut self.terminal, &self.effects, bytes, base, marks) {
            disagreement.log();
        }
        self.log_failure();
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
        // NOTE: a read of the scrollback decompresses the pages that it reads, and
        // that does not change the compression activity. So the next idle steps must
        // run a pass again.
        if scrollback_rows > 0 {
            self.compressed = None;
        }
        snapshot::capture(&self.terminal, scrollback_rows)
    }

    /// libghostty-vt keeps the count of its rows, so this reads no row, and
    /// [`Screen::text`] reads no cell of a full scrollback.
    fn scrollback_rows(&mut self) -> Option<usize> {
        self.terminal.scrollback_rows().ok()
    }

    fn alternate_screen(&mut self) -> bool {
        self.terminal.active_screen().is_ok_and(|screen| screen == ActiveScreen::Alternate)
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
    /// (`kitty-shell-cwd://host/path` or `file://host/path`), not a decoded path. A URL
    /// that is not UTF-8 shows as no working directory.
    fn pwd(&self) -> Option<&str> {
        self.terminal
            .pwd()
            .ok()
            .and_then(|pwd| str::from_utf8(pwd).ok())
            .filter(|pwd| !pwd.is_empty())
    }

    fn idle_delay(&self) -> Option<Duration> {
        Some(COMPRESS_AFTER)
    }

    /// One step of the compression of old scrollback, the work that libghostty-vt
    /// keeps for an idle moment. A full scrollback of 160 columns holds about 7 MiB
    /// and less than 1 MiB when it is compressed. libghostty-vt decompresses a page
    /// again when something reads it, such as a snapshot with scrollback.
    fn idle(&mut self) -> Option<Duration> {
        let activity = self.terminal.compression_activity().ok()?;
        if self.compressed == Some(activity) {
            return None;
        }
        match self.terminal.compress(CompressionMode::Incremental) {
            Ok(CompressionResult::Pending) => Some(COMPRESS_STEP),
            // Done, not possible on this system, or failed: nothing to do until the
            // terminal changes again.
            Ok(_) | Err(_) => {
                self.compressed = self.terminal.compression_activity().ok();
                None
            }
        }
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

/// A factory that restores a screen from a GHOSTSNP `snapshot` on the actor thread.
///
/// The snapshot is a cache of state the recording also holds, so a snapshot that
/// cannot be decoded is logged and the screen starts blank at `size` instead; the
/// owner can replay the recording into it.
pub fn restore_factory(
    snapshot: Vec<u8>,
    size: Size,
    config: GhosttyConfig,
) -> impl FnOnce() -> GhosttyScreen + Send + 'static {
    move || {
        GhosttyScreen::restore(&snapshot, &config)
            .or_else(|err| {
                tracing::warn!(
                    error = &err as &(dyn std::error::Error + 'static),
                    "the ghostty screen starts blank because its snapshot could not be restored"
                );
                GhosttyScreen::new(size, &config)
            })
            .unwrap_or_else(|err| not_started(&err))
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

fn rgb([r, g, b]: [u8; 3]) -> Rgb {
    Rgb { r, g, b }
}

#[cfg(test)]
mod tests;
