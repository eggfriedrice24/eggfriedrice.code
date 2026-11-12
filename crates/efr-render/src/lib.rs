//! Markdown to ANSI for efr.
//!
//! - [`Renderer`]: the streaming renderer. A reply's markdown is pushed in pieces as it
//!   arrives; complete blocks are committed once, to be written to scrollback and
//!   never rewritten, and the block still arriving is the live zone, a string the
//!   caller redraws in place ([`Update`]).
//! - [`render`]: a whole document at once, identical to what a [`Renderer`] commits
//!   for the same text in any pieces.
//! - [`render_trace`]: one muted line for a tool call trace or reasoning.
//! - [`Preload`]: loads the syntax data of code blocks ahead, for a caller that runs
//!   it on a thread of its own so that the first code block does not wait in a frame.
//! - [`diff_rows`]: a unified diff as painted rows of a fixed width, for a caller that
//!   lays the diff out itself.
//! - [`RenderOptions`]: width, [`ColourMode`], [`Palette`], [`Theme`] or
//!   [`CodeTheme`], hyperlinks, [`WidthMethod`], and whether the output is a terminal
//!   at all (when it is not, markdown passes through unchanged).
//! - [`Role`] and [`Palette`]: every colour goes through a role; the default palette
//!   uses the terminal's 16 colours. [`RenderOptions::paint`] paints the CLI's own
//!   lines in a role, [`RenderOptions::paint_with`] adds [`Attrs`] such as bold or
//!   reverse, and [`RenderOptions::fill`] gives the background of a background role
//!   (`code.bg`, `diff.add.bg`, `diff.remove.bg`) for a row filled with `ESC [ K`.
//! - [`Glyphs`]: the marks of the CLI's own lines, in a Unicode and an ASCII table
//!   ([`RenderOptions::glyphs`]).
//! - [`diff_lines`]: a unified diff as one [`DiffLine`] for each line, with its kind,
//!   its painted sign, its painted rows and the fill of its rows, for a caller that
//!   lays the diff out itself.
//! - [`text_width`] and [`display_width`]: columns by code point or by grapheme
//!   cluster, as the terminal counts them. [`display_rows`] and [`last_rows`]: the
//!   rows of a painted line as the terminal wraps it, and its last rows.
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
mod diff_lines;
mod diff_rows;
#[cfg(test)]
mod elements;
mod error;
mod glyph;
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

pub use code::Preload;
pub use code_theme::CodeTheme;
pub use diff_lines::{DiffLine, DiffLineKind, diff_lines};
pub use diff_rows::diff_rows;
pub use error::RenderError;
pub use glyph::Glyphs;
pub use options::{ColourMode, RenderOptions, Theme};
pub use palette::{Palette, Role};
pub use renderer::{Renderer, Update, render};
pub use style::{Attrs, Colour};
pub use trace::render_trace;
pub use width::{WidthMethod, display_rows, display_width, last_rows, text_width};
