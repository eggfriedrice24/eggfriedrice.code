//! The screen model boundary.
//!
//! - [`Screen`]: the trait a terminal backend implements; it has no `Send` bound.
//! - [`ScreenSink`]: where a backend reports PTY replies, bells and title changes.
//!
//! Allowed dependencies: `efr-protocol` and `efr-stdx`. What does not belong here: a
//! terminal emulator (`efr-screen-vt100`, `efr-screen-ghostty`), PTY IO and shell
//! state (`efr-shell`), and the choice of backend (`efr-daemon`).

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod error;
mod screen;
mod shell_marks;
mod sink;

pub use error::ScreenError;
pub use screen::Screen;
pub use shell_marks::{
    ClickMode, PromptKind, SemanticPromptEvent, ShellMark, ShellMarkKind, ShellMarkScanner,
};
pub use sink::ScreenSink;
