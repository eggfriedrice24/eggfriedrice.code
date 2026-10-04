//! Where each shell's screen, and each capture screen, comes from.

use std::fmt;

use efr_holder::Size;
use efr_screen::{ScreenError, ScreenEvents, ScreenHandle};

/// Starts the screen of a new shell, and the short-lived capture screens that read a
/// command's output.
///
/// The daemon implements it with `ScreenActor::spawn` and the factory of the backend
/// it chose (`efr_screen_vt100::factory` or the ghostty one), so this crate never names
/// a backend. `name` is the screen thread's name: `screen-<conversation short id>` for
/// a shell's screen, `replay-<conversation short id>` for a capture screen, which is
/// shut down as soon as the output is read.
///
/// A screen should keep at least 1000 rows of scrollback, as both backends do: a
/// capture screen whose scrollback reaches that many is taken to have lost its oldest
/// rows, and the output is replayed again in smaller pieces.
pub trait ScreenFactory: Send + Sync + fmt::Debug {
    /// Starts one screen of `size`.
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError>;
}
