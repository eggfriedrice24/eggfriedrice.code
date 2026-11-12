//! The rows of one painted line as the terminal wraps it: where each row starts, with
//! the colours and the link that are open there, and the column after its last cell.
//!
//! A line that shares its start with a line laid out before is laid out again only from
//! the row before the one where the two part. The same walk finds the first cell that
//! can differ ([`Spot`]): a redraw starts to write there and leaves the cells before it
//! on the screen.
//!
//! The cells are counted as [`efr_render::display_rows`] counts them: by code point or
//! by grapheme cluster ([`WidthMethod`]), a character of no width joins the cell before
//! it, and a wide character that does not fit at the end of a row starts the next row.

use efr_render::{WidthMethod, text_width};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthChar as _;

/// The colours and the link that are open at a point of a painted line, as the escape
/// sequences that open them again. Colours go back to the last reset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pen {
    colours: String,
    link: String,
}

impl Pen {
    /// Takes the escape sequence `sequence` into the pen.
    fn apply(&mut self, sequence: &str) {
        if let Some(params) = sequence.strip_prefix("\x1b[").and_then(|s| s.strip_suffix('m')) {
            if params.is_empty() || params == "0" {
                self.colours.clear();
            } else {
                self.colours.push_str(sequence);
            }
        } else if let Some(body) = sequence.strip_prefix("\x1b]8;") {
            let body = body.trim_end_matches(['\x07', '\\']).trim_end_matches('\x1b');
            let target = body.split_once(';').map_or("", |(_, target)| target);
            if target.is_empty() {
                self.link.clear();
            } else {
                sequence.clone_into(&mut self.link);
            }
        }
    }

    /// Writes the sequences that open the pen again.
    pub(crate) fn push_to(&self, out: &mut String) {
        out.push_str(&self.colours);
        out.push_str(&self.link);
    }
}

/// Where a row of a line starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RowStart {
    /// The byte offset of the row's first character.
    pub(crate) at: usize,
    /// What is open there.
    pub(crate) pen: Pen,
}

/// One line laid out at a width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    /// Where each row starts; the first row starts at the start of the line.
    pub(crate) starts: Vec<RowStart>,
    /// The column after the last cell of the last row.
    pub(crate) end: usize,
}

impl Layout {
    /// The rows the line takes: at least one.
    pub(crate) fn rows(&self) -> usize {
        self.starts.len()
    }
}

/// The first cell of a line that can differ from the line laid out before: where it
/// stands, and where a write of the rest of the line starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spot {
    /// The row of the line, from 0.
    pub(crate) row: usize,
    /// The column. A wide character that the terminal moves to the next row stands at
    /// the end of the row before it: a write from there leaves no old cell in the gap.
    /// The end of a full row when no cell differs.
    pub(crate) col: usize,
    /// The byte offset to write from.
    pub(crate) at: usize,
    /// What must be open there.
    pub(crate) pen: Pen,
}

/// Lays out `line`, without its newline, at `width` columns.
pub(crate) fn layout(line: &str, width: usize, method: WidthMethod) -> Layout {
    walk(line, width, method, &[RowStart::default()], None).0
}

/// Lays out `line` again, without its newline, when `old` is the layout at the same width
/// of a line with the same first `same` bytes, and finds the first cell that can differ.
pub(crate) fn relayout(
    line: &str,
    old: &Layout,
    same: usize,
    width: usize,
    method: WidthMethod,
) -> (Layout, Spot) {
    let mut same = same.min(line.len());
    while !line.is_char_boundary(same) {
        same -= 1;
    }
    // NOTE: the row before the last one that starts before `same` holds no character at
    // or after `same`, also not the next character, which could make its last cluster
    // longer. So its cells, and where it starts, are the same in both lines.
    let last = old.starts.iter().rposition(|start| start.at < same).unwrap_or(0);
    let from = last.saturating_sub(1);
    let (layout, spot) = walk(line, width, method, &old.starts[..=from], Some(same));
    match spot {
        Some(spot) => (layout, spot),
        None => unreachable!("a walk that looks for a spot always finds one"),
    }
}

/// One cell: its bytes in the line and its width.
#[derive(Debug, Clone, Copy)]
struct Cell {
    start: usize,
    end: usize,
    width: usize,
}

/// Lays out `line` from the last of `known`, the starts of its first rows, and with
/// `find` finds the first cell whose next cell does not start before that byte offset:
/// the first cell that can differ from the old line's.
fn walk(
    line: &str,
    width: usize,
    method: WidthMethod,
    known: &[RowStart],
    find: Option<usize>,
) -> (Layout, Option<Spot>) {
    let width = width.max(1);
    let mut starts = known.to_vec();
    let Some(from) = starts.last().cloned() else {
        unreachable!("a walk starts at a known row");
    };
    let mut pen = from.pen;
    let (cells, escapes) = scan(line, from.at, method);
    let mut row = starts.len() - 1;
    let mut col = 0;
    let mut spot = None;
    let mut escape = 0;
    for (index, cell) in cells.iter().enumerate() {
        while let Some((start, end)) = escapes.get(escape).filter(|(start, _)| *start < cell.start)
        {
            pen.apply(&line[*start..*end]);
            escape += 1;
        }
        let wraps = cell.width > 0 && col > 0 && col + cell.width > width;
        // NOTE: a character of no width joins the cell before it also across an
        // escape sequence, so a cell stays only when the next cell starts in the
        // shared text: what stands between the two can have changed.
        let next = cells.get(index + 1).map_or(line.len(), |next| next.start);
        if spot.is_none() && find.is_some_and(|same| next >= same) {
            let (row, col) = if wraps && col >= width { (row + 1, 0) } else { (row, col) };
            spot = Some(Spot { row, col, at: cell.start, pen: pen.clone() });
        }
        if wraps {
            row += 1;
            col = 0;
            starts.push(RowStart { at: cell.start, pen: pen.clone() });
        }
        col += cell.width;
    }
    if spot.is_none() && find.is_some() {
        for (start, end) in &escapes[escape..] {
            pen.apply(&line[*start..*end]);
        }
        spot = Some(Spot { row, col, at: line.len(), pen });
    }
    (Layout { starts, end: col }, spot)
}

/// The cells of `line` from byte `from`, and the byte ranges of its escape sequences
/// (CSI and OSC) there. A character of no width joins the cell before it.
fn scan(line: &str, from: usize, method: WidthMethod) -> (Vec<Cell>, Vec<(usize, usize)>) {
    let mut chars: Vec<(usize, char)> = Vec::new();
    let mut escapes = Vec::new();
    let mut iter = line[from..].char_indices().map(|(at, c)| (at + from, c)).peekable();
    while let Some((at, c)) = iter.next() {
        if c != '\x1b' {
            chars.push((at, c));
            continue;
        }
        let mut end = at + 1;
        match iter.next() {
            // A CSI sequence ends with its final byte, 0x40 to 0x7e.
            Some((_, '[')) => {
                end += 1;
                for (at, c) in iter.by_ref() {
                    end = at + c.len_utf8();
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // An OSC sequence ends with BEL or with ST (ESC \).
            Some((_, ']')) => {
                end += 1;
                while let Some((at, c)) = iter.next() {
                    end = at + c.len_utf8();
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        if let Some((at, c)) = iter.next() {
                            end = at + c.len_utf8();
                        }
                        break;
                    }
                }
            }
            Some((at, c)) => end = at + c.len_utf8(),
            None => {}
        }
        escapes.push((at, end));
    }
    let mut cells: Vec<Cell> = Vec::with_capacity(chars.len());
    match method {
        WidthMethod::CodePoint => {
            for (at, c) in chars {
                let end = at + c.len_utf8();
                let width = c.width().unwrap_or(0);
                match cells.last_mut() {
                    Some(last) if width == 0 => last.end = end,
                    _ => cells.push(Cell { start: at, end, width }),
                }
            }
        }
        WidthMethod::Grapheme => {
            // A cluster can go on across a colour change, as it does on the terminal: join
            // the text, then map each cluster back to the line.
            let text: String = chars.iter().map(|(_, c)| *c).collect();
            let mut index = Vec::with_capacity(chars.len());
            let mut joined = 0;
            for (_, c) in &chars {
                index.push(joined);
                joined += c.len_utf8();
            }
            let mut next = 0;
            for (offset, cluster) in text.grapheme_indices(true) {
                while index.get(next).is_some_and(|at| *at < offset) {
                    next += 1;
                }
                let first = next;
                while index.get(next).is_some_and(|at| *at < offset + cluster.len()) {
                    next += 1;
                }
                let (start, _) = chars[first];
                let (last, c) = chars[next - 1];
                let width = text_width(cluster, WidthMethod::Grapheme);
                let end = last + c.len_utf8();
                match cells.last_mut() {
                    Some(cell) if width == 0 => cell.end = end,
                    _ => cells.push(Cell { start, end, width }),
                }
            }
        }
    }
    (cells, escapes)
}

#[cfg(test)]
mod tests;
