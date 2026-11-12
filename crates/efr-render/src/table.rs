//! Tables: columns sized to their widest cell when the table fits the width, and one
//! record per row when it does not. Column widths need every row, so a table is laid
//! out only when it is complete.
//!
//! A grid has its columns apart by spaces, the header in bold, and one `muted` rule
//! under the header as wide as the table. A record has one line for each column: the
//! header in `muted` in a column of its own, two spaces, the value; the first value of
//! a record in bold, and a blank line between records (TM-Markdown 1, 4, 5). No line
//! ends in padding.

use pulldown_cmark::Alignment;

use crate::style::{Line, Span, Style, line_width, push_span};

/// The spaces between two columns of a grid.
const COLUMN_GAP: usize = 3;
/// The spaces between the label and the value of a record.
const RECORD_GAP: usize = 2;

#[derive(Debug, Default)]
pub(crate) struct Table {
    aligns: Vec<Alignment>,
    /// The rule under the header and the labels of a record: the `muted` role.
    muted: Style,
    /// One cell of the rule under the header.
    rule: &'static str,
    head: Vec<Line>,
    rows: Vec<Vec<Line>>,
    row: Vec<Line>,
}

impl Table {
    pub(crate) fn new(aligns: Vec<Alignment>, muted: Style, rule: &'static str) -> Table {
        Table { aligns, muted, rule, ..Table::default() }
    }

    pub(crate) fn start_row(&mut self) {
        self.row.clear();
    }

    pub(crate) fn push_cell(&mut self, cell: Line) {
        self.row.push(cell);
    }

    pub(crate) fn end_head(&mut self) {
        self.head = std::mem::take(&mut self.row);
    }

    pub(crate) fn end_row(&mut self) {
        self.rows.push(std::mem::take(&mut self.row));
    }

    /// The table's lines for `width` columns. An empty line stands for a blank line
    /// between records.
    pub(crate) fn layout(&self, width: usize) -> Vec<Line> {
        let columns = self.rows.iter().map(Vec::len).fold(self.head.len(), usize::max);
        if columns == 0 {
            return Vec::new();
        }
        let widths: Vec<usize> = (0..columns)
            .map(|column| {
                std::iter::once(&self.head)
                    .chain(&self.rows)
                    .filter_map(|row| row.get(column))
                    .map(|cell| line_width(cell))
                    .fold(1, usize::max)
            })
            .collect();
        let total = widths.iter().sum::<usize>() + COLUMN_GAP * (columns - 1);
        if total <= width { self.grid(&widths, total) } else { self.records(columns) }
    }

    fn grid(&self, widths: &[usize], total: usize) -> Vec<Line> {
        let mut lines = Vec::with_capacity(self.rows.len() + 2);
        if !self.head.is_empty() {
            lines.push(self.grid_row(&self.head, widths, true));
            lines.push(vec![Span::new(self.rule.repeat(total), self.muted)]);
        }
        for row in &self.rows {
            lines.push(self.grid_row(row, widths, false));
        }
        lines
    }

    fn grid_row(&self, row: &[Line], widths: &[usize], header: bool) -> Line {
        let mut line = Vec::new();
        for (column, width) in widths.iter().enumerate() {
            if column > 0 {
                push_span(&mut line, Span::plain(" ".repeat(COLUMN_GAP)));
            }
            let cell = row.get(column).map_or(&[][..], Vec::as_slice);
            let pad = width.saturating_sub(line_width(cell));
            let (left, right) = match self.aligns.get(column) {
                Some(Alignment::Right) => (pad, 0),
                Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                _ => (0, pad),
            };
            push_span(&mut line, Span::plain(" ".repeat(left)));
            for span in cell {
                push_span(&mut line, if header { emphasise(span) } else { span.clone() });
            }
            push_span(&mut line, Span::plain(" ".repeat(right)));
        }
        line
    }

    fn records(&self, columns: usize) -> Vec<Line> {
        let mut lines = Vec::new();
        if self.rows.is_empty() {
            for cell in &self.head {
                lines.push(cell.iter().map(emphasise).collect());
            }
            return lines;
        }
        let label_width = self.head.iter().map(|cell| line_width(cell)).max().unwrap_or(0);
        for (index, row) in self.rows.iter().enumerate() {
            if index > 0 {
                lines.push(Vec::new());
            }
            for column in 0..columns {
                let mut line = Vec::new();
                let label = self.head.get(column).map_or(&[][..], Vec::as_slice);
                if label_width > 0 {
                    for span in label {
                        push_span(&mut line, Span { style: self.muted, ..span.clone() });
                    }
                    let pad = label_width - line_width(label) + RECORD_GAP;
                    push_span(&mut line, Span::plain(" ".repeat(pad)));
                }
                for span in row.get(column).into_iter().flatten() {
                    let span = if column == 0 { emphasise(span) } else { span.clone() };
                    push_span(&mut line, span);
                }
                lines.push(line);
            }
        }
        lines
    }
}

fn emphasise(span: &Span) -> Span {
    Span { style: span.style.bold(), ..span.clone() }
}

#[cfg(test)]
mod tests;
