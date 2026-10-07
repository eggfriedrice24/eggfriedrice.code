//! Markdown to ANSI for efr.
//!
//! - [`Renderer`]: the streaming renderer. A reply's markdown is pushed in pieces as it
//!   arrives; complete blocks are committed once, to be written to scrollback and
//!   never rewritten, and the block still arriving is the live zone, a string the
//!   caller redraws in place ([`Update`]).
//! - [`render`]: a whole document at once, identical to what a [`Renderer`] commits
//!   for the same text in any pieces.
//! - [`render_trace`]: one muted line for a tool call trace or reasoning.
//! - [`RenderOptions`]: width, [`ColourMode`], [`Palette`], [`Theme`] or
//!   [`CodeTheme`], hyperlinks, [`WidthMethod`], and whether the output is a terminal
//!   at all (when it is not, markdown passes through unchanged).
//! - [`Role`] and [`Palette`]: every colour goes through a role; the default palette
//!   uses the terminal's 16 colours. [`RenderOptions::paint`] paints the CLI's own
//!   lines in a role.
//! - [`text_width`] and [`display_width`]: columns by code point or by grapheme
//!   cluster, as the terminal counts them.
//!
//! Headings, emphasis, inline code, lists, task items, quotes, tables, links (as OSC 8
//! hyperlinks, `file://` for paths), code blocks with syntax colours and unified diffs
//! are rendered. Top-level prose is not hard-wrapped, so the terminal reflows it on
//! resize; indented blocks are wrapped to keep their indent.
//!
//! Allowed dependencies: no workspace crate. What does not belong here: any IO,
//! terminal control (cursor movement, erasing, synchronized output) and reading the
//! environment; the CLI owns those and passes a [`RenderOptions`]. A `.tmTheme` file
//! comes in as bytes.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod block;
mod code;
mod code_theme;
#[cfg(test)]
mod elements;
mod error;
mod highlight;
mod link;
mod options;
mod outline;
mod palette;
mod renderer;
mod style;
mod table;
mod trace;
mod width;
mod wrap;

pub use code_theme::CodeTheme;
pub use error::RenderError;
pub use options::{ColourMode, RenderOptions, Theme};
pub use palette::{Palette, Role};
pub use renderer::{Renderer, Update, render};
pub use style::Colour;
pub use trace::render_trace;
pub use width::{WidthMethod, display_width, text_width};
