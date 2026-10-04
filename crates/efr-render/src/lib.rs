//! Markdown to ANSI for efr.
//!
//! - [`RenderOptions`]: width, [`ColourMode`], [`Theme`], hyperlinks, and whether the
//!   output is a terminal at all (when it is not, markdown passes through unchanged).
//!
//! Allowed dependencies: no workspace crate. What does not belong here: any IO,
//! terminal control (cursor movement, erasing, synchronized output) and reading the
//! environment; the CLI owns those and passes a [`RenderOptions`].

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod block;
mod code;
mod error;
mod highlight;
mod link;
mod options;
mod outline;
mod style;
mod table;
mod wrap;

pub use error::RenderError;
pub use options::{ColourMode, RenderOptions, Theme};
