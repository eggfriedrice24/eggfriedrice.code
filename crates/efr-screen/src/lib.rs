//! The screen model boundary.
//!
//! - [`Screen`]: the trait a terminal backend implements (`efr-screen-vt100`,
//!   `efr-screen-ghostty`). It has no `Send` bound, because libghostty-vt types are
//!   neither `Send` nor `Sync`.
//! - [`ScreenSink`]: where a backend reports PTY replies, bells and title changes.
//! - [`ScreenActor`]: runs one screen on its own named std thread. [`ScreenHandle`]
//!   commands it and [`ScreenEvents`] carries what it produces as [`ScreenEvent`]s;
//!   those two are the only types other crates hold.
//! - [`ShellMarkScanner`]: finds OSC 133 and OSC 7 [`ShellMark`]s with their
//!   recording offsets. The actor runs it on every chunk before the backend sees it,
//!   so every backend reports the same marks.
//! - [`ScreenCapture`] and [`row_text`]: snapshots in the wire form of
//!   `efr_protocol::ScreenSnapshot`, and the text a row shows.
//! - The wire types a backend builds ([`ScreenSnapshot`], [`RowCells`], [`Cell`],
//!   [`Color`], [`Cursor`], [`Size`]), re-exported from `efr-protocol` so a backend
//!   crate implements [`Screen`] with `efr-screen` as its only workspace dependency.
//! - `conformance` (feature `conformance`, for test targets only): the suite every
//!   backend runs over the fixtures in `fixtures/`.
//!
//! Allowed dependencies: `efr-protocol` and `efr-stdx`. What does not belong here: a
//! terminal emulator (`efr-screen-vt100`, `efr-screen-ghostty`), PTY IO and shell
//! state (`efr-shell`), and the choice of backend (`efr-daemon`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod actor;
#[cfg(feature = "conformance")]
pub mod conformance;
mod error;
#[cfg(test)]
mod fake;
mod screen;
mod shell_marks;
mod sink;
mod snapshot;

pub use actor::{SCREEN_STACK_SIZE, ScreenActor, ScreenEvent, ScreenEvents, ScreenHandle};
pub use efr_protocol::{Cell, Color, Cursor, RowCells, ScreenSnapshot, Size};
pub use error::ScreenError;
pub use screen::Screen;
pub use shell_marks::{
    ClickMode, PromptKind, SemanticPromptEvent, ShellMark, ShellMarkKind, ShellMarkScanner,
};
pub use sink::ScreenSink;
pub use snapshot::{ScreenCapture, row_text};
