//! The terminal screen as a client sees it when it attaches to a PTY.
//!
//! The daemon keeps the terminal state (vt100 or libghostty-vt behind `efr-screen`) and
//! sends an attaching client a snapshot of the visible grid plus as much scrollback as
//! it asks for, then raw output. Only the daemon answers terminal queries; a client only
//! renders. Default attributes are left out of the JSON, so a plain cell is
//! `{"text": "a"}`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A terminal size in character cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Size {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// The cursor position in the visible grid, counted from 0 at the top left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Cursor {
    /// The row, from 0 at the top of the visible grid.
    pub row: u16,
    /// The column, from 0 at the left edge.
    pub col: u16,
    /// True when the program has hidden the cursor.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// A cell colour. On the wire: `{"indexed": 1}` or `{"rgb": [255, 128, 0]}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Color {
    /// One of the 256 palette colours; 0 to 15 are the terminal's theme colours.
    Indexed(u8),
    /// A 24-bit colour as red, green and blue.
    Rgb([u8; 3]),
}

/// One character cell.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Cell {
    /// The grapheme in the cell. Empty for a blank cell and for the second cell of a
    /// wide character.
    pub text: String,
    /// The foreground colour; absent means the terminal's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<Color>,
    /// The background colour; absent means the terminal's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<Color>,
    /// Bold text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// Italic text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    /// Underlined text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    /// Foreground and background swapped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inverse: bool,
    /// The first cell of a character that takes two columns.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wide: bool,
}

/// One row of cells.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct RowCells {
    /// The cells from the left edge. Trailing blank cells may be left out.
    pub cells: Vec<Cell>,
    /// True when the row continues on the next row because the line was soft-wrapped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrapped: bool,
}

/// The state of a terminal screen at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenSnapshot {
    /// The size of the visible grid.
    pub size: Size,
    /// The cursor.
    pub cursor: Cursor,
    /// The visible grid from the top row down.
    pub rows: Vec<RowCells>,
    /// Rows above the visible grid, oldest first, as many as the client asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scrollback: Vec<RowCells>,
    /// The window title that the program set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// True while a full-screen program uses the alternate screen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alternate_screen: bool,
}

#[cfg(test)]
mod tests;
