//! Markdown to ANSI for efr.
//!
//! - [`Renderer`]: the streaming renderer. A reply's markdown is pushed in pieces as it
//!   arrives; complete blocks are committed once, to be written to scrollback and
//!   never rewritten, and the block still arriving is the live zone, a string the
//!   caller redraws in place ([`Update`]).
//! - [`render`]: a whole document at once, identical to what a [`Renderer`] commits
//!   for the same text in any pieces.
//! - [`RenderOptions`]: width, [`ColourMode`], [`Theme`], hyperlinks, and whether the
//!   output is a terminal at all (when it is not, markdown passes through unchanged).
//!
//! Headings, emphasis, inline code, lists, task items, quotes, tables, links (as OSC 8
//! hyperlinks, `file://` for paths), code blocks with syntax colours and unified diffs
//! are rendered. Top-level prose is not hard-wrapped, so the terminal reflows it on
//! resize; indented blocks are wrapped to keep their indent.
//!
//! Allowed dependencies: no workspace crate. What does not belong here: any IO,
//! terminal control (cursor movement, erasing, synchronized output) and reading the
//! environment; the CLI owns those and passes a [`RenderOptions`].

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod block;
mod code;
#[cfg(test)]
mod elements;
mod error;
mod highlight;
mod link;
mod options;
mod outline;
mod renderer;
mod style;
mod table;
mod wrap;

pub use error::RenderError;
pub use options::{ColourMode, RenderOptions, Theme};
pub use renderer::{Renderer, Update, render};
