//! Tables: columns sized to their widest cell when the table fits the width, and one
//! record per row (`header: value` lines) when it does not. Column widths need every
//! row, so a table is laid out only when it is complete.

use pulldown_cmark::Alignment;

use crate::style::{Line, Span, Style, line_width, push_span};

const COLUMN_GAP: &str = " \u{2502} ";
const RULE: char = '\u{2500}';
const RULE_CROSS: &str = "\u{2500}\u{253c}\u{2500}";

#[derive(Debug, Default)]
pub(crate) struct Table {
    aligns: Vec<Alignment>,
    head: Vec<Line>,
    rows: Vec<Vec<Line>>,
    row: Vec<Line>,
}

impl Table {
    pub(crate) fn new(aligns: Vec<Alignment>) -> Table {
        Table { aligns, ..Table::default() }
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
        let total = widths.iter().sum::<usize>() + COLUMN_GAP.chars().count() * (columns - 1);
        if total <= width { self.grid(&widths) } else { self.records(columns) }
    }

    fn grid(&self, widths: &[usize]) -> Vec<Line> {
        let mut lines = Vec::with_capacity(self.rows.len() + 2);
        if !self.head.is_empty() {
            lines.push(self.grid_row(&self.head, widths, true));
            let rule: Vec<String> =
                widths.iter().map(|width| RULE.to_string().repeat(*width)).collect();
            lines.push(vec![Span::new(rule.join(RULE_CROSS), Style::PLAIN.dim())]);
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
                push_span(&mut line, Span::new(COLUMN_GAP, Style::PLAIN.dim()));
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
        for (index, row) in self.rows.iter().enumerate() {
            if index > 0 {
                lines.push(Vec::new());
            }
            for column in 0..columns {
                let mut line = Vec::new();
                if let Some(header) = self.head.get(column).filter(|cell| !cell.is_empty()) {
                    for span in header {
                        push_span(&mut line, emphasise(span));
                    }
                    push_span(&mut line, Span::new(":", Style::PLAIN.bold()));
                    push_span(&mut line, Span::plain(" "));
                }
                for span in row.get(column).into_iter().flatten() {
                    push_span(&mut line, span.clone());
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
