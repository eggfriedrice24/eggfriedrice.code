//! A small terminal for this crate's tests, standing in for the real backends, which
//! live in their own crates.
//!
//! It understands printable ASCII, CR, LF, BS, HT and BEL, deferred wrap, scrolling
//! into scrollback, cursor movement (`CUP`, `CUU`, `CUD`, `CUF`, `CUB`), `ED` and
//! `EL`, OSC 0 and 2 titles, and answers DA1 and DSR the way libghostty-vt does. It
//! skips every other escape sequence and drops bytes outside ASCII. It holds an `Rc`,
//! so it is not `Send`: the actor tests prove that a screen needs no `Send`.

use std::collections::VecDeque;
use std::rc::Rc;

use efr_protocol::{Cell, Cursor, RowCells, ScreenSnapshot, Size};

use crate::{Screen, ScreenSink};

/// The answer libghostty-vt gives to a primary device attributes query.
pub(crate) const DA1_REPLY: &[u8] = b"\x1b[?62;22c";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parse {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Skip,
    SkipEscape,
}

#[derive(Debug)]
pub(crate) struct FakeScreen {
    cols: usize,
    rows: usize,
    grid: Vec<Vec<char>>,
    scrollback: VecDeque<Vec<char>>,
    row: usize,
    col: usize,
    /// The cursor sits past the last column; the next printable character wraps.
    pending_wrap: bool,
    title: Option<String>,
    parse: Parse,
    sequence: Vec<u8>,
    _not_send: Rc<()>,
}

impl FakeScreen {
    pub(crate) fn new(size: Size) -> Self {
        let (cols, rows) = (usize::from(size.cols), usize::from(size.rows));
        FakeScreen {
            cols,
            rows,
            grid: vec![vec![' '; cols]; rows],
            scrollback: VecDeque::new(),
            row: 0,
            col: 0,
            pending_wrap: false,
            title: None,
            parse: Parse::Ground,
            sequence: Vec::new(),
            _not_send: Rc::new(()),
        }
    }

    /// A factory for [`crate::ScreenActor::spawn`] and the conformance suite.
    pub(crate) fn factory(size: Size) -> impl FnOnce() -> FakeScreen + Send + 'static {
        move || FakeScreen::new(size)
    }

    fn byte(&mut self, byte: u8, sink: &mut dyn ScreenSink) {
        match self.parse {
            Parse::Ground => self.ground(byte, sink),
            Parse::Escape => {
                self.sequence.clear();
                self.parse = match byte {
                    b'[' => Parse::Csi,
                    b']' => Parse::Osc,
                    b'P' | b'X' | b'^' | b'_' => Parse::Skip,
                    0x1b => Parse::Escape,
                    _ => Parse::Ground,
                };
            }
            Parse::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.parse = Parse::Ground;
                    self.csi(byte, sink);
                } else {
                    self.sequence.push(byte);
                }
            }
            Parse::Osc => match byte {
                0x07 => self.osc(sink),
                0x1b => self.parse = Parse::OscEscape,
                _ => self.sequence.push(byte),
            },
            Parse::OscEscape => {
                if byte == b'\\' {
                    self.osc(sink);
                } else {
                    self.parse = Parse::Ground;
                }
            }
            Parse::Skip => {
                if byte == 0x1b {
                    self.parse = Parse::SkipEscape;
                }
            }
            Parse::SkipEscape => {
                self.parse = if byte == b'\\' { Parse::Ground } else { Parse::Skip };
            }
        }
    }

    fn ground(&mut self, byte: u8, sink: &mut dyn ScreenSink) {
        match byte {
            0x07 => sink.bell(),
            0x08 => {
                self.col = self.col.saturating_sub(1);
                self.pending_wrap = false;
            }
            b'\t' => self.col = ((self.col / 8 + 1) * 8).min(self.cols.saturating_sub(1)),
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\r' => {
                self.col = 0;
                self.pending_wrap = false;
            }
            0x1b => self.parse = Parse::Escape,
            0x20..=0x7e => self.print(char::from(byte)),
            _ => {}
        }
    }

    fn print(&mut self, c: char) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        if self.pending_wrap {
            self.col = 0;
            self.line_feed();
        }
        self.grid[self.row][self.col] = c;
        if self.col + 1 == self.cols {
            self.pending_wrap = true;
        } else {
            self.col += 1;
        }
    }

    fn line_feed(&mut self) {
        self.pending_wrap = false;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else if self.rows > 0 {
            let top = self.grid.remove(0);
            self.scrollback.push_back(top);
            self.grid.push(vec![' '; self.cols]);
        }
    }

    fn params(&self) -> Vec<usize> {
        String::from_utf8_lossy(&self.sequence)
            .split(';')
            .map(|param| param.parse().unwrap_or(0))
            .collect()
    }

    fn csi(&mut self, last: u8, sink: &mut dyn ScreenSink) {
        if self.sequence.first().is_some_and(|byte| matches!(byte, b'?' | b'>' | b'=')) {
            return;
        }
        let params = self.params();
        let first = params.first().copied().unwrap_or(0);
        let count = first.max(1);
        self.pending_wrap = false;
        match last {
            b'c' if first == 0 => sink.pty_reply(DA1_REPLY),
            b'n' if first == 5 => sink.pty_reply(b"\x1b[0n"),
            b'n' if first == 6 => {
                let report = format!("\x1b[{};{}R", self.row + 1, self.col + 1);
                sink.pty_reply(report.as_bytes());
            }
            b'H' | b'f' => {
                let col = params.get(1).copied().unwrap_or(0).max(1);
                self.row = (count - 1).min(self.rows.saturating_sub(1));
                self.col = (col - 1).min(self.cols.saturating_sub(1));
            }
            b'A' => self.row = self.row.saturating_sub(count),
            b'B' => self.row = (self.row + count).min(self.rows.saturating_sub(1)),
            b'C' => self.col = (self.col + count).min(self.cols.saturating_sub(1)),
            b'D' => self.col = self.col.saturating_sub(count),
            b'J' => self.erase_display(first),
            b'K' => self.erase_line(self.row, first),
            _ => {}
        }
    }

    fn erase_line(&mut self, row: usize, mode: usize) {
        let line = &mut self.grid[row];
        let range = match mode {
            0 => self.col..line.len(),
            1 => 0..(self.col + 1).min(line.len()),
            _ => 0..line.len(),
        };
        line[range].fill(' ');
    }

    fn erase_display(&mut self, mode: usize) {
        let rows = match mode {
            0 => {
                self.erase_line(self.row, 0);
                self.row + 1..self.rows
            }
            1 => {
                self.erase_line(self.row, 1);
                0..self.row
            }
            _ => 0..self.rows,
        };
        for row in rows {
            self.grid[row].fill(' ');
        }
    }

    fn osc(&mut self, sink: &mut dyn ScreenSink) {
        self.parse = Parse::Ground;
        let body = String::from_utf8_lossy(&self.sequence).into_owned();
        if let Some(title) = body.strip_prefix("0;").or_else(|| body.strip_prefix("2;")) {
            self.title = Some(title.to_owned());
            sink.title_changed(title);
        }
    }

    fn cells(line: &[char]) -> RowCells {
        let cells = line
            .iter()
            .map(|&c| Cell {
                text: if c == ' ' { String::new() } else { c.to_string() },
                ..Cell::default()
            })
            .collect();
        RowCells { cells, wrapped: false }
    }
}

impl Screen for FakeScreen {
    fn feed(&mut self, bytes: &[u8], sink: &mut dyn ScreenSink) {
        for &byte in bytes {
            self.byte(byte, sink);
        }
    }

    fn resize(&mut self, cols: u16, rows: u16, _sink: &mut dyn ScreenSink) {
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        self.grid.resize(rows, vec![' '; cols]);
        for line in &mut self.grid {
            line.resize(cols, ' ');
        }
        self.cols = cols;
        self.rows = rows;
        self.row = self.row.min(rows.saturating_sub(1));
        self.col = self.col.min(cols.saturating_sub(1));
        self.pending_wrap = false;
    }

    fn snapshot(&mut self, scrollback_rows: usize) -> ScreenSnapshot {
        let skip = self.scrollback.len().saturating_sub(scrollback_rows);
        ScreenSnapshot {
            size: Size {
                cols: u16::try_from(self.cols).unwrap_or(u16::MAX),
                rows: u16::try_from(self.rows).unwrap_or(u16::MAX),
            },
            cursor: self.cursor(),
            rows: self.grid.iter().map(|line| Self::cells(line)).collect(),
            scrollback: self.scrollback.iter().skip(skip).map(|line| Self::cells(line)).collect(),
            title: self.title.clone(),
            alternate_screen: false,
        }
    }

    fn row(&self, index: usize) -> RowCells {
        self.grid.get(index).map(|line| Self::cells(line)).unwrap_or_default()
    }

    fn cursor(&self) -> Cursor {
        Cursor {
            row: u16::try_from(self.row).unwrap_or(u16::MAX),
            col: u16::try_from(self.col).unwrap_or(u16::MAX),
            hidden: false,
        }
    }

    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn pwd(&self) -> Option<&str> {
        None
    }
}
