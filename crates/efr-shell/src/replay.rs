//! A command's output as the screen shows it.
//!
//! Output that only prints text, colours, carriage-return progress bars and
//! backspaces reads the same through the byte cleaner (`capture.rs`) as on a screen,
//! and the cleaner keeps the tabs that a screen turns into spaces, so such output
//! stays on the cleaner and costs no screen. Output that moves the cursor (a
//! multi-line progress display redrawn with cursor-up, a full-screen program) makes
//! sense only on a screen: it is replayed on a capture screen from the session's own
//! [`ScreenFactory`], at the shell's width, with rows enough for typical output, and
//! read back with its scrollback. The screen lives for one replay, so a finished run
//! keeps no thread.
//!
//! A row that scrolled off the top is out of reach of every cursor movement, so the
//! scrollback is final; but a backend keeps only so much of it. A replay whose
//! scrollback reaches [`TRUSTED_SCROLLBACK`] may have lost its oldest rows, so it is
//! replayed again in two halves split at a line end.
//!
//! When the capture screen cannot start or stops early, the cleaner reads the bytes.
//!
//! The live tail of a running command ([`Replayer::tail`]) follows the same rule, on the
//! end of the output only: a screen of the shell's own size, read without its
//! scrollback, so it costs one screen of rows however long the output is.

use bytes::Bytes;
use efr_holder::Size;
use efr_protocol::{RowCells, ScreenSnapshot, Seq};
use efr_screen::{ScreenError, row_text};

use crate::ScreenFactory;
use crate::capture::{Captured, Kept, clean, join, skip_escape};

/// The most rows a capture screen has, unless the shell itself has more. A backend
/// allocates the rows up front (vt100 takes 32 bytes a cell, so 200 rows of 160
/// columns are 1 MiB); taller output scrolls into the scrollback.
pub(crate) const MAX_ROWS: u16 = 200;

/// The rows of scrollback that every backend keeps at least: vt100 keeps 1000
/// (`efr_screen_vt100::DEFAULT_SCROLLBACK_ROWS`) and ghostty 5000. A replay whose
/// scrollback holds this many may have lost its oldest rows.
pub(crate) const TRUSTED_SCROLLBACK: usize = 1000;

/// The line that stands for what a full-screen program showed.
pub(crate) const FULL_SCREEN_NOTE: &str =
    "[a full-screen program ran here; only what it left on the main screen is shown]";

/// Leaves the alternate screen. CAN comes first and ends any escape sequence that the
/// output stopped in, so the switch is not read as part of it.
const LEAVE_ALTERNATE: &[u8] = b"\x18\x1b[?1049l";

/// Turns a run's kept output into text, on a capture screen where it needs one.
pub(crate) struct Replayer<'a> {
    screens: &'a dyn ScreenFactory,
    /// The capture screen's thread name.
    name: String,
    /// The shell's terminal size: the capture screen's width and its least height.
    size: Size,
}

impl<'a> Replayer<'a> {
    pub(crate) fn new(screens: &'a dyn ScreenFactory, name: String, size: Size) -> Self {
        Replayer { screens, name, size }
    }

    /// The output as text. When its middle was dropped, the head and the tail are read
    /// apart, so no screen holds more than the kept bytes, and the marker line joins
    /// them.
    pub(crate) async fn render(&self, kept: &Kept) -> Captured {
        let head = self.text(&kept.head).await;
        let text = if kept.truncated() {
            join(&head, kept.dropped, &self.text(&kept.tail).await)
        } else {
            head
        };
        Captured { text, truncated: kept.truncated(), bytes: kept.bytes }
    }

    /// One part of the output as text.
    pub(crate) async fn text(&self, bytes: &Bytes) -> String {
        let scan = Scan::of(bytes);
        if !scan.wants_screen() {
            return clean(bytes);
        }
        match self.replay(bytes, scan.full_screen).await {
            Ok(text) => text,
            Err(error) => {
                tracing::debug!(
                    screen = %self.name,
                    %error,
                    "the capture screen failed; the output is cleaned byte by byte instead"
                );
                clean(bytes)
            }
        }
    }

    /// The end of a running command's output as text, for its live tail: the rows that
    /// a screen of the shell's size shows after `bytes`, without the rows that
    /// scrolled off it, or the cleaner's text when `bytes` need no screen or the
    /// screen fails. A full-screen program still running is ended on the copy, as
    /// [`text`](Self::text) does, so the tail shows the main screen and the note.
    pub(crate) async fn tail(&self, bytes: &Bytes) -> String {
        let scan = Scan::of(bytes);
        if !scan.wants_screen() {
            return clean(bytes);
        }
        match self.replay_piece(bytes, self.size, 0).await {
            Ok(snapshot) => {
                let mut text = screen_text(&snapshot, true);
                if scan.full_screen {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(FULL_SCREEN_NOTE);
                }
                text
            }
            Err(error) => {
                tracing::debug!(
                    screen = %self.name,
                    %error,
                    "the live tail's screen failed; the tail is cleaned byte by byte instead"
                );
                clean(bytes)
            }
        }
    }

    /// The text of `bytes` replayed on capture screens, in more than one piece when the
    /// scrollback of one may have overflowed.
    async fn replay(&self, bytes: &Bytes, full_screen: bool) -> Result<String, ScreenError> {
        // The pieces still to replay, the next one last.
        let mut pieces = vec![bytes.clone()];
        let mut text = String::new();
        while let Some(piece) = pieces.pop() {
            let last = pieces.is_empty();
            let size = Size { cols: self.size.cols, rows: rows_for(&piece, self.size) };
            let snapshot = self.replay_piece(&piece, size, usize::MAX).await?;
            if snapshot.scrollback.len() < TRUSTED_SCROLLBACK {
                text.push_str(&screen_text(&snapshot, last));
                continue;
            }
            if let Some(at) = middle_line_end(&piece) {
                pieces.push(piece.slice(at..));
                pieces.push(piece.slice(..at));
                continue;
            }
            // One line taller than the screen and its scrollback together: only the
            // cleaner keeps all of it.
            tracing::debug!(screen = %self.name, "a line overflowed the capture screen");
            text.push_str(&clean(&piece));
        }
        if full_screen {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(FULL_SCREEN_NOTE);
        }
        Ok(text)
    }

    /// Replays `piece` on a capture screen of its own of `size` and reads it back with
    /// at most `scrollback` rows of its scrollback. The screen stops when this returns,
    /// and also when the caller drops the future, because its last handle goes with it.
    async fn replay_piece(
        &self,
        piece: &Bytes,
        size: Size,
        scrollback: usize,
    ) -> Result<ScreenSnapshot, ScreenError> {
        let (screen, events) = self.screens.spawn(&self.name, size)?;
        // Nobody answers this screen's terminal queries or hears its bells. With the
        // stream gone, the actor drops its events instead of waiting for a reader.
        drop(events);
        let read = async {
            screen.feed(piece.clone(), Seq::ZERO).await?;
            // What a full-screen program shows is gone once it ends; a program cut off
            // by the end of the bytes is ended here, so that its main screen shows.
            if Scan::of(piece).alternate_at_end {
                let at = Seq::new(piece.len() as u64);
                screen.feed(Bytes::from_static(LEAVE_ALTERNATE), at).await?;
            }
            screen.snapshot(scrollback).await
        };
        let capture = read.await;
        // A screen that already stopped has nothing left to stop.
        let _ = screen.shutdown().await;
        Ok(capture?.snapshot)
    }
}

/// What a part of the output does to a terminal beyond printing text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scan {
    /// It moves the cursor, erases more than the rest of its line, scrolls, inserts or
    /// deletes, saves or restores the cursor, or changes how text lands: the byte
    /// cleaner cannot follow it.
    pub(crate) needs_screen: bool,
    /// It sets a scroll region.
    pub(crate) scroll_region: bool,
    /// It switched to the alternate screen.
    pub(crate) full_screen: bool,
    /// The alternate screen is up at its end.
    pub(crate) alternate_at_end: bool,
}

impl Scan {
    /// True when the bytes read right only on a screen. A screen drops the lines that
    /// scroll out of a scroll region, where the cleaner keeps them; losing output is
    /// worse than showing a status line twice, so a scroll region stays on the cleaner.
    pub(crate) fn wants_screen(self) -> bool {
        self.needs_screen && !self.scroll_region
    }

    pub(crate) fn of(bytes: &[u8]) -> Self {
        let mut scan = Scan::default();
        let mut at = 0;
        while let Some(offset) = bytes[at..].iter().position(|&byte| byte == 0x1b) {
            let start = at + offset;
            at = skip_escape(bytes, start);
            match bytes.get(start + 1) {
                Some(b'[') => scan.csi(&bytes[(start + 2).min(at)..at]),
                // DECSC, DECRC, IND, NEL, RI and RIS.
                Some(b'7' | b'8' | b'D' | b'E' | b'M' | b'c') => scan.needs_screen = true,
                // DECALN fills the screen with `E`.
                Some(b'#') if bytes.get(start + 2) == Some(&b'8') => scan.needs_screen = true,
                _ => {}
            }
        }
        scan
    }

    /// Takes one CSI sequence after its `ESC [`: parameters, intermediates and the
    /// final byte.
    fn csi(&mut self, body: &[u8]) {
        let Some((&final_byte, params)) = body.split_last() else {
            return;
        };
        // Cut off by the end of the bytes.
        if !(0x40..=0x7e).contains(&final_byte) {
            return;
        }
        let private = params.first().is_some_and(|byte| (b'<'..=b'?').contains(byte));
        match final_byte {
            b'h' | b'l' => self.mode(params, final_byte == b'h'),
            b'J' => self.needs_screen = true,
            // Erasing to the end of the line follows a carriage return in a progress
            // bar, and the cleaner already lets the text after a carriage return
            // replace the line.
            b'K' => {
                let params = params.strip_prefix(b"?").unwrap_or(params);
                self.needs_screen |= !matches!(params, b"" | b"0");
            }
            b'r' if !private => {
                self.needs_screen = true;
                self.scroll_region |= !params.is_empty();
            }
            // Queries, keyboard modes and the like print nothing.
            _ if private => {}
            b'A'..=b'I'
            | b'L'
            | b'M'
            | b'P'
            | b'S'
            | b'T'
            | b'X'
            | b'Z'
            | b'@'
            | b'`'
            | b'a'
            | b'b'
            | b'd'
            | b'e'
            | b'f'
            | b's'
            | b'u' => self.needs_screen = true,
            _ => {}
        }
    }

    /// Takes the modes of one SM or RM sequence (`h` sets, `l` resets).
    fn mode(&mut self, params: &[u8], set: bool) {
        let (private, modes) = match params.strip_prefix(b"?") {
            Some(modes) => (true, modes),
            None => (false, params),
        };
        for mode in modes.split(|&byte| byte == b';') {
            match (private, mode) {
                (true, b"47" | b"1047" | b"1049") => {
                    self.needs_screen = true;
                    self.full_screen |= set;
                    self.alternate_at_end = set;
                }
                // 132 columns, origin mode, autowrap and left and right margins; insert
                // mode and newline mode.
                (true, b"3" | b"6" | b"7" | b"69") | (false, b"4" | b"20") => {
                    self.needs_screen = true;
                }
                _ => {}
            }
        }
    }
}

/// The capture screen's height for `bytes`: as many rows as they can fill, at least the
/// shell's own height, so a program that addresses rows by number finds them where it
/// expects, and at most [`MAX_ROWS`] (or the shell's height when that is more).
fn rows_for(bytes: &[u8], shell: Size) -> u16 {
    let floor = shell.rows.max(1);
    let ceiling = MAX_ROWS.max(floor);
    u16::try_from(rows_needed(bytes, shell.cols)).unwrap_or(u16::MAX).clamp(floor, ceiling)
}

/// At least the rows that `bytes` fill on a screen `cols` wide: one for each line feed
/// (LF, VT, FF, IND and NEL) and one for each `cols` columns printed, counting every
/// byte of a character (no character is wider than its bytes) and eight for a tab.
pub(crate) fn rows_needed(bytes: &[u8], cols: u16) -> usize {
    let cols = usize::from(cols.max(1));
    let mut feeds = 1;
    let mut columns = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            0x1b => {
                if matches!(bytes.get(at + 1), Some(b'D' | b'E')) {
                    feeds += 1;
                }
                at = skip_escape(bytes, at);
                continue;
            }
            b'\n' | 0x0b | 0x0c => feeds += 1,
            b'\t' => columns += 8,
            0x00..=0x1f | 0x7f => {}
            _ => columns += 1,
        }
        at += 1;
    }
    feeds + columns / cols
}

/// The index just after the line feed nearest the middle of `bytes`, such that both
/// halves hold something; `None` when there is no such line feed.
pub(crate) fn middle_line_end(bytes: &[u8]) -> Option<usize> {
    let middle = bytes.len() / 2;
    let before = bytes[..middle].iter().rposition(|&byte| byte == b'\n').map(|at| at + 1);
    let after = bytes[middle..].iter().position(|&byte| byte == b'\n').map(|at| middle + at + 1);
    [before, after]
        .into_iter()
        .flatten()
        .filter(|&at| at > 0 && at < bytes.len())
        .min_by_key(|&at| at.abs_diff(middle))
}

/// The text of a replayed screen: the scrollback, then the visible rows, a soft-wrapped
/// row joined to the next, each line without trailing blanks, and no blank lines at
/// the end. A piece that more pieces follow keeps the blank lines above its cursor,
/// where the next piece goes on, and ends with a line break.
pub(crate) fn screen_text(snapshot: &ScreenSnapshot, last: bool) -> String {
    let rows: Vec<&RowCells> = snapshot.scrollback.iter().chain(&snapshot.rows).collect();
    let cols = usize::from(snapshot.size.cols);
    let cursor = snapshot.scrollback.len() + usize::from(snapshot.cursor.row);
    // Each line with the index of its first row.
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (index, row) in rows.iter().enumerate() {
        let (start, mut text) = open.take().unwrap_or_else(|| (index, String::new()));
        match rows.get(index + 1) {
            Some(next) if row.wrapped => {
                text.push_str(&wrapped_text(row, cols, next));
                open = Some((start, text));
            }
            _ => {
                text.push_str(&row_text(row));
                text.truncate(text.trim_end_matches(' ').len());
                lines.push((start, text));
            }
        }
    }
    while lines.last().is_some_and(|(start, text)| text.is_empty() && (last || *start >= cursor)) {
        lines.pop();
    }
    let ends_a_line = !last && !lines.is_empty();
    let mut text = lines.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n");
    if ends_a_line {
        text.push('\n');
    }
    text
}

/// The text of a soft-wrapped row: every column, because the line goes on in the next
/// row and a blank at the wrap is part of it. The snapshot leaves trailing blank cells
/// out, so they come back as spaces, except the one that a wide character leaves in
/// the last column when it does not fit there and starts the next row instead.
fn wrapped_text(row: &RowCells, cols: usize, next: &RowCells) -> String {
    let mut text = String::new();
    let mut after_wide = false;
    for cell in &row.cells {
        if cell.text.is_empty() {
            if !after_wide {
                text.push(' ');
            }
        } else {
            text.push_str(&cell.text);
        }
        after_wide = cell.wide;
    }
    // A wide character in the last cells lost its blank second cell to the trim.
    let covered = row.cells.len() + usize::from(row.cells.last().is_some_and(|cell| cell.wide));
    let mut blanks = cols.saturating_sub(covered);
    if blanks > 0 && next.cells.first().is_some_and(|cell| cell.wide) {
        blanks -= 1;
    }
    text.extend(std::iter::repeat_n(' ', blanks));
    text
}

#[cfg(test)]
mod tests;
