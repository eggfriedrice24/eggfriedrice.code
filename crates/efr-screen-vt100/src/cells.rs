//! From vt100's cells to the wire cells.

use efr_screen::{Cell, Color, RowCells};

/// Row `index` of what vt100 shows: the visible grid, or the scrollback rows the
/// view is scrolled to. vt100 pads every row to the full width; the actor trims the
/// trailing blanks later.
pub(crate) fn row(screen: &vt100::Screen, index: u16) -> RowCells {
    let (_, cols) = screen.size();
    let cells = (0..cols).map_while(|col| screen.cell(index, col)).map(cell).collect();
    RowCells { cells, wrapped: screen.row_wrapped(index) }
}

/// One cell. vt100 already leaves the second cell of a wide character without text,
/// as the wire form wants. Dim has no wire field, so it is dropped.
pub(crate) fn cell(cell: &vt100::Cell) -> Cell {
    Cell {
        text: cell.contents().to_owned(),
        fg: color(cell.fgcolor()),
        bg: color(cell.bgcolor()),
        bold: cell.bold(),
        italic: cell.italic(),
        underline: cell.underline(),
        inverse: cell.inverse(),
        wide: cell.is_wide(),
    }
}

/// The terminal's default colour is the absence of a colour on the wire.
pub(crate) fn color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(red, green, blue) => Some(Color::Rgb([red, green, blue])),
    }
}

#[cfg(test)]
mod tests;
