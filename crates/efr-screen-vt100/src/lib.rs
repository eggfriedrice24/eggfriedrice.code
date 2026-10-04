//! The vt100 screen backend: [`Vt100Screen`] implements `efr_screen::Screen` over the
//! vt100 crate, and [`factory`] builds one on its screen thread for
//! `efr_screen::ScreenActor::spawn`.
//!
//! vt100 is pure Rust, so this is the backend that needs no Zig: every test in the
//! workspace and every build without libghostty-vt runs on it. It goes through the
//! same actor and the same conformance suite as `efr-screen-ghostty`; only the
//! factory differs. Shell marks (OSC 133 and OSC 7) do not come from here: the actor's
//! scanner finds them before the bytes reach the backend.
//!
//! Allowed dependencies: `efr-screen` only; the wire types come through its
//! re-exports. What does not belong here: the actor, the scanner and the snapshot
//! normalisation (`efr-screen`), answering terminal queries (vt100 answers none, and
//! the fixtures say so in their `differs` lists), and choosing a backend
//! (`efr-daemon`).

mod cells;
mod recorder;
mod screen;
#[cfg(test)]
mod test_sink;

pub use screen::{DEFAULT_SCROLLBACK_ROWS, Vt100Screen, factory};
