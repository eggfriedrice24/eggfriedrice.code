//! Where each shell's screen comes from.

use std::fmt;

use efr_holder::Size;
use efr_screen::{ScreenError, ScreenEvents, ScreenHandle};

/// Starts the screen of a new shell.
///
/// The daemon implements it with `ScreenActor::spawn` and the factory of the backend
/// it chose (`efr_screen_vt100::factory` or the ghostty one), so this crate never names
/// a backend. `name` is the screen thread's name, `screen-<conversation short id>`.
pub trait ScreenFactory: Send + Sync + fmt::Debug {
    /// Starts one screen of `size`.
    fn spawn(&self, name: &str, size: Size) -> Result<(ScreenHandle, ScreenEvents), ScreenError>;
}
