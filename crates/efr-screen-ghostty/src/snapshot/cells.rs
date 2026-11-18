//! One libghostty-vt row as wire cells.
//!
//! A query that libghostty-vt fails (it should not, for a point inside the grid)
//! yields a blank cell rather than an error: a snapshot is a picture, and one wrong
//! cell is better than none.

use efr_screen::{Cell, Color, RowCells};
use libghostty_vt::Terminal;
use libghostty_vt::grid_ref::GridRef;
use libghostty_vt::point::Point;
use libghostty_vt::screen::{CellContentTag, CellWide, Row};
use libghostty_vt::style::{StyleColor, Underline};

/// Enough room for almost every grapheme; a longer cluster is read again with the
/// size libghostty-vt asks for.
const GRAPHEME_CHARS: usize = 8;

/// The `cols` cells of the row that `point(x)` addresses for every column `x`.
pub(super) fn row(
    terminal: &Terminal<'_, '_>,
    cols: u16,
    point: impl Fn(u16) -> Point,
) -> RowCells {
    let mut wrapped = false;
    let cells = (0..cols)
        .map(|x| match terminal.grid_ref(point(x)) {
            Ok(grid) => {
                if x == 0 {
                    wrapped = grid.row().and_then(Row::is_wrapped).unwrap_or(false);
                }
                cell(&grid)
            }
            Err(_) => Cell::default(),
        })
        .collect();
    RowCells { cells, wrapped }
}

/// True when no cell of the row that `point(x)` addresses shows text other than a
/// space, the rule of `efr_screen::row_text` for an empty line. It stops at the
/// first cell with text.
pub(super) fn blank(terminal: &Terminal<'_, '_>, cols: u16, point: impl Fn(u16) -> Point) -> bool {
    (0..cols).all(|x| {
        let Ok(grid) = terminal.grid_ref(point(x)) else {
            return true;
        };
        let wide = grid.cell().and_then(|raw| raw.wide()).unwrap_or(CellWide::Narrow);
        let shows = matches!(wide, CellWide::Narrow | CellWide::Wide)
            && grid.cell().and_then(|raw| raw.has_text()).unwrap_or(false);
        !shows || text(&grid) == " "
    })
}

/// True when the row that `point(x)` addresses is soft-wrapped into the next row.
pub(super) fn wraps(terminal: &Terminal<'_, '_>, point: impl Fn(u16) -> Point) -> bool {
    terminal
        .grid_ref(point(0))
        .and_then(|grid| grid.row())
        .and_then(Row::is_wrapped)
        .unwrap_or(false)
}

/// One cell: its grapheme and the attributes the wire format carries. The second
/// cell of a wide character, and the spacer a wide character leaves at the end of
/// a wrapped row, have no text.
pub(super) fn cell(grid: &GridRef<'_>) -> Cell {
    let Ok(raw) = grid.cell() else {
        return Cell::default();
    };
    let mut cell = Cell::default();
    let wide = raw.wide().unwrap_or(CellWide::Narrow);
    cell.wide = wide == CellWide::Wide;
    if wide != CellWide::SpacerTail
        && wide != CellWide::SpacerHead
        && raw.has_text().unwrap_or(false)
    {
        cell.text = text(grid);
    }
    if let Ok(style) = grid.style() {
        cell.fg = color(style.fg_color);
        cell.bg = color(style.bg_color);
        cell.bold = style.bold;
        cell.italic = style.italic;
        cell.underline = style.underline != Underline::None;
        cell.inverse = style.inverse;
    }
    // A cell erased with a background colour holds the colour itself, not a style.
    cell.bg = match raw.content_tag() {
        Ok(CellContentTag::BgColorPalette) => {
            raw.bg_color_palette().ok().map(|index| Color::Indexed(index.0))
        }
        Ok(CellContentTag::BgColorRgb) => {
            raw.bg_color_rgb().ok().map(|rgb| Color::Rgb([rgb.r, rgb.g, rgb.b]))
        }
        _ => cell.bg,
    };
    cell
}

/// The grapheme cluster of a cell: its first code point and any that combine with
/// it.
fn text(grid: &GridRef<'_>) -> String {
    let mut chars = [char::default(); GRAPHEME_CHARS];
    match grid.graphemes(&mut chars) {
        Ok(len) => chars.iter().take(len).collect(),
        Err(libghostty_vt::Error::OutOfSpace { required }) => {
            let mut chars = vec![char::default(); required];
            grid.graphemes(&mut chars)
                .map_or_else(|_| String::new(), |len| chars.iter().take(len).collect())
        }
        Err(_) => String::new(),
    }
}

fn color(color: StyleColor) -> Option<Color> {
    match color {
        StyleColor::None => None,
        StyleColor::Palette(index) => Some(Color::Indexed(index.0)),
        StyleColor::Rgb(rgb) => Some(Color::Rgb([rgb.r, rgb.g, rgb.b])),
    }
}

#[cfg(test)]
mod tests;
