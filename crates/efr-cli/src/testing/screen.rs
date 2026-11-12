//! A terminal screen for the frame tests, stricter than [`Grid`](super::Grid): cells with
//! their colours and link, wide characters, a cursor that waits at the end of a full
//! row until the next character wraps it, a screen of fixed height that scrolls its top
//! row into the scrollback, a cursor-up that stops at the top row, the erases (which
//! fill with the colours that are open, as a terminal with BCE does) and the delete of
//! rows (`CSI M`). A wide character that does not fit at the end of a row leaves that
//! cell as it was, as xterm does. Any other sequence fails the test.

use efr_render::{WidthMethod, text_width};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthChar as _;

/// One cell: the text that stands in it and what is open there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cell {
    text: String,
    pen: String,
    /// The right half of a wide character.
    tail: bool,
}

impl Cell {
    fn blank(pen: &str) -> Cell {
        Cell { text: " ".to_owned(), pen: pen.to_owned(), tail: false }
    }

    fn is_blank(&self) -> bool {
        self.text == " " && self.pen.is_empty() && !self.tail
    }
}

/// What the screen shows: every row from the first of the scrollback, without the blank
/// cells at their ends, and the cursor (row from the first of the scrollback, column,
/// and whether it waits at the end of a full row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shot {
    rows: Vec<Vec<Cell>>,
    cursor: (usize, usize, bool),
}

/// A screen `cols` wide and `rows` high, with its scrollback.
#[derive(Debug)]
pub(crate) struct Screen {
    cols: usize,
    rows: usize,
    method: WidthMethod,
    /// The scrollback, then the screen: its last `rows` rows.
    lines: Vec<Vec<Cell>>,
    row: usize,
    col: usize,
    pending: bool,
    colours: String,
    link: String,
}

impl Screen {
    pub(crate) fn new(cols: u16, rows: u16, method: WidthMethod) -> Screen {
        let rows = usize::from(rows);
        Screen {
            cols: usize::from(cols),
            rows,
            method,
            lines: vec![Vec::new(); rows],
            row: 0,
            col: 0,
            pending: false,
            colours: String::new(),
            link: String::new(),
        }
    }

    pub(crate) fn shot(&self) -> Shot {
        let mut rows: Vec<Vec<Cell>> = self
            .lines
            .iter()
            .map(|line| {
                let mut line = line.clone();
                while line.last().is_some_and(Cell::is_blank) {
                    line.pop();
                }
                line
            })
            .collect();
        let top = self.top();
        // Empty rows at the bottom of the screen count only up to the cursor.
        while rows.len() > top + self.row + 1 && rows.last().is_some_and(Vec::is_empty) {
            rows.pop();
        }
        Shot { rows, cursor: (top + self.row, self.col, self.pending) }
    }

    /// The text of each row from the first of the scrollback, without blanks at the
    /// end, and without empty rows at the bottom.
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .lines
            .iter()
            .map(|line| {
                let text: String =
                    line.iter().filter(|cell| !cell.tail).map(|cell| cell.text.as_str()).collect();
                text.trim_end().to_owned()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// The row on the screen and the column of the cursor.
    pub(crate) fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// The text of each row, for a message.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index == self.top() {
                out.push_str("---- screen\n");
            }
            for cell in line.iter().filter(|cell| !cell.tail) {
                out.push_str(&cell.text);
            }
            out.push('\n');
        }
        out
    }

    fn top(&self) -> usize {
        self.lines.len() - self.rows
    }

    fn pen(&self) -> String {
        format!("{}{}", self.colours, self.link)
    }

    fn line(&mut self) -> &mut Vec<Cell> {
        let at = self.top() + self.row;
        &mut self.lines[at]
    }

    /// Writes `text`; each newline is a carriage return and a newline, as `ONLCR` makes it.
    pub(crate) fn write(&mut self, text: &str) {
        // The characters since the last control, each with what is open at it: a cluster
        // goes on across a colour change, as it does on the terminal.
        let mut run: Vec<(char, String)> = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut last = ' ';
                        for c in chars.by_ref() {
                            if ('\u{40}'..='\u{7e}').contains(&c) {
                                last = c;
                                break;
                            }
                            params.push(c);
                        }
                        if last == 'm' {
                            if params.is_empty() || params == "0" {
                                self.colours.clear();
                            } else {
                                self.colours.push_str(&format!("\x1b[{params}m"));
                            }
                        } else {
                            self.flush(&mut run);
                            self.csi(&params, last);
                        }
                    }
                    Some(']') => {
                        let mut body = String::new();
                        while let Some(c) = chars.next() {
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' && chars.next_if_eq(&'\\').is_some() {
                                break;
                            }
                            body.push(c);
                        }
                        if let Some(rest) = body.strip_prefix("8;") {
                            let target = rest.split_once(';').map_or("", |(_, target)| target);
                            self.link = if target.is_empty() {
                                String::new()
                            } else {
                                format!("\x1b]8;{rest}")
                            };
                        }
                    }
                    other => panic!("an escape the screen does not know: {other:?}"),
                },
                '\r' => {
                    self.flush(&mut run);
                    self.col = 0;
                    self.pending = false;
                }
                '\n' => {
                    self.flush(&mut run);
                    self.newline();
                    self.col = 0;
                }
                c => run.push((c, self.pen())),
            }
        }
        self.flush(&mut run);
    }

    fn newline(&mut self) {
        self.pending = false;
        if self.row + 1 == self.rows {
            self.lines.push(Vec::new());
        } else {
            self.row += 1;
        }
    }

    fn flush(&mut self, run: &mut Vec<(char, String)>) {
        if run.is_empty() {
            return;
        }
        let run = std::mem::take(run);
        match self.method {
            WidthMethod::CodePoint => {
                for (c, pen) in run {
                    self.put(&c.to_string(), c.width().unwrap_or(0), &pen);
                }
            }
            WidthMethod::Grapheme => {
                let text: String = run.iter().map(|(c, _)| *c).collect();
                let mut offsets = Vec::with_capacity(run.len());
                let mut at = 0;
                for (c, _) in &run {
                    offsets.push(at);
                    at += c.len_utf8();
                }
                let mut next = 0;
                for (at, cluster) in text.grapheme_indices(true) {
                    while offsets[next] < at {
                        next += 1;
                    }
                    let pen = run[next].1.clone();
                    self.put(cluster, text_width(cluster, WidthMethod::Grapheme), &pen);
                }
            }
        }
    }

    /// Puts one cell of `width` columns at the cursor.
    fn put(&mut self, text: &str, width: usize, pen: &str) {
        if width == 0 {
            // It joins the cell before the cursor.
            let col = if self.pending { self.col } else { self.col.saturating_sub(1) };
            let line = self.line();
            if let Some(cell) = line.get_mut(col) {
                cell.text.push_str(text);
            }
            return;
        }
        if self.pending {
            self.newline();
            self.col = 0;
        }
        if self.col + width > self.cols {
            self.newline();
            self.col = 0;
        }
        let (col, cols) = (self.col, self.cols);
        let line = self.line();
        if line.len() < col + width {
            line.resize(col + width, Cell::blank(""));
        }
        // A wide character that is cut in half loses its other half.
        if line[col].tail && col > 0 {
            line[col - 1] = Cell::blank("");
        }
        let end = col + width;
        if end < line.len() && line[end].tail {
            line[end] = Cell::blank("");
        }
        line[col] = Cell { text: text.to_owned(), pen: pen.to_owned(), tail: false };
        if width == 2 {
            line[col + 1] = Cell { text: String::new(), pen: pen.to_owned(), tail: true };
        }
        self.col = end;
        if self.col >= cols {
            self.col = cols - 1;
            self.pending = true;
        }
    }

    fn erase_line_from(&mut self, col: usize) {
        let (cols, pen) = (self.cols, self.colours.clone());
        let line = self.line();
        line.truncate(col);
        if !pen.is_empty() {
            line.resize(cols, Cell::blank(&pen));
        }
    }

    fn csi(&mut self, params: &str, last: char) {
        if params.starts_with('?') && matches!(last, 'h' | 'l') {
            return;
        }
        let count = params.parse::<usize>().unwrap_or(1).max(1);
        self.pending = false;
        match (last, params) {
            ('A', _) => self.row = self.row.saturating_sub(count),
            ('B', _) => self.row = (self.row + count).min(self.rows - 1),
            ('C', _) => self.col = (self.col + count).min(self.cols - 1),
            ('D', _) => self.col = self.col.saturating_sub(count),
            ('J', "" | "0") => {
                self.erase_line_from(self.col);
                let row = self.row;
                for below in row + 1..self.rows {
                    self.row = below;
                    self.erase_line_from(0);
                }
                self.row = row;
            }
            ('K', "" | "0") => self.erase_line_from(self.col),
            ('K', "2") => self.erase_line_from(0),
            ('M', _) => {
                let at = self.top() + self.row;
                let count = count.min(self.rows - self.row);
                self.lines.drain(at..at + count);
                let pen = self.colours.clone();
                for _ in 0..count {
                    let blank = if pen.is_empty() {
                        Vec::new()
                    } else {
                        vec![Cell::blank(&pen); self.cols]
                    };
                    self.lines.push(blank);
                }
                self.col = 0;
            }
            other => panic!("a sequence the screen does not know: {other:?}"),
        }
    }
}
