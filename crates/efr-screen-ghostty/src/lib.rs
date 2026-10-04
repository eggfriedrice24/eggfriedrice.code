//! The libghostty-vt screen backend.
//!
//! - [`GhosttyScreen`]: `efr_screen::Screen` over a libghostty-vt `Terminal`. It
//!   answers terminal queries (DA, DSR, DECRQM, OSC 10 and 11) through the sink,
//!   and exports and restores GHOSTSNP snapshots.
//! - [`factory`], [`factory_with`] and [`restore_factory`]: the `Send` factories that
//!   `efr_screen::ScreenActor::spawn` runs on the screen thread. libghostty-vt types
//!   are neither `Send` nor `Sync`, so a screen is built there and never leaves.
//! - [`GhosttyConfig`]: scrollback and the default colours.
//! - [`GhosttyError`]: what can fail; a feed never does.
//!
//! This is the only crate that links libghostty-vt, so the only one whose build needs
//! Zig, at the version `docs/ghostty-pin.md` pins. It is not a default workspace
//! member.
//!
//! Allowed dependencies: `efr-screen`. What does not belong here: the actor, the
//! scanner and the wire types (`efr-screen`), the choice of backend (`efr-daemon`), PTY
//! IO (`efr-shell`).

mod effects;
mod error;
mod snapshot;
mod terminal;
#[cfg(test)]
mod testing;

pub use error::GhosttyError;
pub use terminal::{GhosttyConfig, GhosttyScreen, factory, factory_with, restore_factory};
