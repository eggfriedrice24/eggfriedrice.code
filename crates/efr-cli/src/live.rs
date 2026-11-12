//! Redrawing the live zone in place: the part of a reply that is still arriving.
//!
//! Committed output is written once and scrolls away like any other output. The live
//! zone sits below it and changes on every update. A frame writes only what changed,
//! all inside synchronized output (mode 2026) so the terminal shows the result without
//! flicker. It keeps the layout of each line on the screen ([`mod@layout`]): where each of
//! its rows starts. A line that is the same on the same rows stays as it is. A line
//! that changed only in its last row is written from its first cell that changed, and
//! the rest of that row is erased when the old row was longer: text that streams at the
//! end of a line writes only its new characters, and a tick writes only the status row.
//! At the first line that changed in any other way, the frame moves to its first cell
//! that changed, erases to the end of the screen, and writes from there to the end.
//! Committed output takes the place of the lines at the top of the live zone, so a
//! block that commits writes nothing when it stands on the screen as it was.
//!
//! The rows are counted as the terminal counts columns: by code point, or by grapheme
//! cluster in Ghostty ([`WidthMethod`]). A ZWJ emoji, a flag or a variation selector
//! counted the other way would put the cursor one row too high or too low. A wide
//! character that does not fit at the end of a row starts the next row, as the
//! terminal wraps it. A write from the middle of a line opens its colours and its link
//! again first.
//!
//! The live zone is kept smaller than the screen: rows that scrolled off the top can
//! no longer be reached by moving the cursor up, so they could never be erased. When
//! it would be taller, only its last rows are shown: the last lines that fit whole,
//! and above them the last rows of the line before them. So a paragraph line taller
//! than the screen shows its newest rows, not nothing. When such a line grows by rows,
//! the frame deletes as many rows at the top of the live zone (`CSI M`): the rows
//! below move up, nothing above the live zone moves, and only the new rows are written.
//!
//! After a resize, the frame erases the whole live zone and writes it again: the
//! terminal wraps the old lines again at the new width, so their height is measured
//! again at that width. A resize that makes the old live zone taller than the new
//! screen makes the terminal push its top rows into the scrollback, where no cursor
//! move reaches them: they stay there.
//!
//! The live zone can end with a status row and the input row of the turn below it (the
//! tail). While the input row shows, the cursor waits in it, where the next typed
//! character goes ([`Cursor`]). Every live line ends with a newline, so without a
//! cursor in the tail the cursor waits at the start of the row below the live zone.

mod layout;

use std::fmt::Write as _;

use efr_render::{WidthMethod, display_rows};

use self::layout::{Layout, Spot, layout, relayout};
use crate::terminal::Size;

/// Starts synchronized output: the terminal holds the screen until the end mark.
const BEGIN_SYNC: &str = "\x1b[?2026h";
/// Ends synchronized output.
const END_SYNC: &str = "\x1b[?2026l";
/// The width when the terminal does not report one, the same fallback as
/// `efr_render::RenderOptions`.
const FALLBACK_WIDTH: u16 = 80;

/// Where the cursor waits after a redraw: in the tail of the live zone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Cursor {
    /// The line of the tail, from 0.
    pub(crate) line: usize,
    /// The column in that line, from 0.
    pub(crate) column: usize,
}

/// A line of the live zone as it stands on the screen.
#[derive(Debug, Clone)]
struct Line {
    /// The whole line with its newline, also the rows of it that the clip cut off.
    text: String,
    /// Its rows at the width of the live zone.
    layout: Layout,
    /// The rows of it above the top of the live zone, which the clip cut off: only the
    /// first line can have some.
    hidden: usize,
}

/// The live zone as it stands on the screen.
#[derive(Debug, Default)]
pub(crate) struct LiveZone {
    /// How the terminal counts the width of text.
    method: WidthMethod,
    /// The text on the screen now, after clipping.
    shown: String,
    /// Where the cursor waits in the tail; `None` when it waits below the live zone.
    cursor: Option<Cursor>,
    /// The lines of the tail from the cursor's line on, which the cursor is above the
    /// row below the live zone.
    below: String,
    /// The rows it took when it was written.
    rows: usize,
    /// The width it was written at.
    width: u16,
    /// The size of the screen it was written on.
    size: Size,
    /// The lines on the screen, from the top of the live zone, laid out at `width`.
    lines: Vec<Line>,
    /// The index of the first of `lines` in the live text, after the lines that the
    /// clip left out.
    first: usize,
    /// Each frame erases the whole live zone and writes it again: the reference that
    /// the tests compare the frames with.
    #[cfg(test)]
    whole: bool,
}

/// A line of the next frame: committed output or the live zone.
#[derive(Debug)]
struct Next<'a> {
    text: &'a str,
    layout: Layout,
    hidden: usize,
    /// The text equals the line on the screen in the same place.
    same: bool,
    /// The first cell that can differ from the line on the screen in the same place.
    spot: Option<Spot>,
}

impl Next<'_> {
    fn shown_rows(&self) -> usize {
        self.layout.rows() - self.hidden
    }

    /// Writes the line from the top of the live zone on: the rows that the clip cut off
    /// are left out, and the colours and the link open where it starts are opened again.
    fn push_shown(&self, out: &mut String) {
        match self.layout.starts.get(self.hidden).filter(|_| self.hidden > 0) {
            Some(start) => {
                start.pen.push_to(out);
                out.push_str(&self.text[start.at..]);
            }
            None => out.push_str(self.text),
        }
    }
}

/// The cursor of a frame while it writes, in rows from the top of the old live zone.
struct Pencil<'a> {
    out: &'a mut String,
    row: usize,
    /// The column; `None` when it is not known, such as after a full row.
    col: Option<usize>,
    width: usize,
}

impl Pencil<'_> {
    /// Moves the cursor to `col` of `row`, both from 0.
    fn go(&mut self, row: usize, col: usize) {
        if self.row == row && self.col == Some(col) {
            return;
        }
        match self.col {
            Some(at) if self.row == row && at < self.width && at < col => {
                let _ = write!(self.out, "\x1b[{}C", col - at);
            }
            // NOTE: each row down to the row below the live zone is on the screen, so a
            // newline there moves down and scrolls nothing.
            _ if col == 0 && row > self.row && row - self.row <= 4 => {
                for _ in self.row..row {
                    self.out.push('\n');
                }
            }
            _ => {
                self.out.push('\r');
                if row < self.row {
                    let _ = write!(self.out, "\x1b[{}A", self.row - row);
                } else if row > self.row {
                    let _ = write!(self.out, "\x1b[{}B", row - self.row);
                }
                if col > 0 {
                    let _ = write!(self.out, "\x1b[{col}C");
                }
            }
        }
        self.row = row;
        self.col = Some(col);
    }

    /// The cursor stands after `col` columns of the row, or at its end.
    fn after(&mut self, col: usize) {
        self.col = (col < self.width).then_some(col);
    }
}

/// What a frame did, for the probe of the stream bench.
#[derive(Debug, Default)]
struct Done {
    /// Rows of the old live zone that the frame wrote over, erased or deleted.
    erased: usize,
    /// The lines that the frame wrote in place, by their index in the frame.
    in_place: Vec<usize>,
    /// The frame wrote to the end of the screen, erased or deleted rows.
    more: bool,
}

impl LiveZone {
    /// An empty live zone on a terminal that counts widths by `method`.
    pub(crate) fn new(method: WidthMethod) -> LiveZone {
        LiveZone { method, ..LiveZone::default() }
    }

    /// A live zone whose frames erase the whole live zone and write it again, as each
    /// frame did before frames wrote only what changed.
    #[cfg(test)]
    pub(crate) fn whole(method: WidthMethod) -> LiveZone {
        LiveZone { method, whole: true, ..LiveZone::default() }
    }

    /// The bytes that write `committed` once and show `live` in place of the live zone.
    /// Empty when nothing would change.
    #[cfg(test)]
    pub(crate) fn redraw(&mut self, committed: &str, live: &str, size: Size) -> String {
        self.draw(committed, live, "", &Tail::default(), size)
    }

    /// Forgets the live zone on the screen: the next draw starts a new one at the
    /// cursor and moves nothing up. After a stop, the shell's lines stand below the
    /// old live zone, which stays as it was.
    pub(crate) fn forget(&mut self) {
        *self = LiveZone {
            method: self.method,
            #[cfg(test)]
            whole: self.whole,
            ..LiveZone::default()
        };
    }

    /// True when each frame must erase the whole live zone and write it again.
    fn always_whole(&self) -> bool {
        #[cfg(test)]
        {
            self.whole
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    /// The bytes that write `committed` once and show `body`, the status row `status`
    /// (one line with its newline, or empty) and `tail` in place of the live zone, as
    /// one string for one write. The cursor then waits where `tail` says, or below the
    /// live zone. Empty when nothing would change.
    pub(crate) fn draw(
        &mut self,
        committed: &str,
        body: &str,
        status: &str,
        tail: &Tail,
        size: Size,
    ) -> String {
        let width = effective_width(size);
        let method = self.method;
        let live = format!("{body}{status}{}", tail.text);
        let texts: Vec<&str> = live.split_inclusive('\n').collect();
        let committed_texts: Vec<&str> = committed.split_inclusive('\n').collect();
        // NOTE: a line keeps its layout while the width stays; the old line in the same
        // place on the screen gives it, or the start that the two share.
        let reuse = width == self.width && !self.lines.is_empty();
        let (lines, first, rows) = self.lay_out(&texts, committed_texts.len(), width, reuse, size);
        let mut shown = String::with_capacity(live.len());
        for line in &lines {
            line.push_shown(&mut shown);
        }
        // A tail that does not fit whole on the screen gets no cursor.
        let cursor = tail.cursor.filter(|_| shown.ends_with(&*tail.text));
        // An empty live zone looks the same at any width.
        if committed.is_empty()
            && shown == self.shown
            && cursor == self.cursor
            && (width == self.width || shown.is_empty())
        {
            #[cfg(test)]
            crate::testing::probe::zone(crate::testing::probe::Zone {
                empty: true,
                live_rows: rows,
                hidden_rows: rows_of(&live, width, method).saturating_sub(rows),
                ..Default::default()
            });
            return String::new();
        }
        let below = match cursor {
            Some(cursor) => tail.text.split_inclusive('\n').skip(cursor.line).collect(),
            None => String::new(),
        };
        let below_rows = rows_of(&below, width, method);
        let mut out = String::from(BEGIN_SYNC);
        // NOTE: a committed line or live text without its newline would leave the cursor
        // in the row, so the rows after it could not be counted.
        let whole = self.always_whole()
            || size != self.size
            || size.rows == 0
            || !committed.is_empty() && !committed.ends_with('\n')
            || !live.is_empty() && !live.ends_with('\n');
        let committed_rows = rows_of(committed, width, method);
        let (done, lines) = if whole {
            self.write_whole(&mut out, committed, &shown, width);
            if let Some(cursor) = cursor {
                if below_rows > 0 {
                    let _ = write!(out, "\x1b[{below_rows}A");
                }
                out.push('\r');
                if cursor.column > 0 {
                    let _ = write!(out, "\x1b[{}C", cursor.column);
                }
            }
            (Done { erased: self.rows, more: true, ..Done::default() }, lines)
        } else {
            let mut next: Vec<Next<'_>> = Vec::with_capacity(committed_texts.len() + lines.len());
            for (index, text) in committed_texts.iter().enumerate() {
                next.push(lay(text, self.lines.get(index), width, method));
            }
            // NOTE: the lines were laid out against the old lines in the same place when the
            // clip leaves out as many lines as before; else against the lines that stand
            // in their place now.
            let realign = first != self.first;
            for line in lines {
                next.push(if realign {
                    let mut again = lay(line.text, self.lines.get(next.len()), width, method);
                    again.hidden = line.hidden;
                    again
                } else {
                    line
                });
            }
            let (done, row, col) = self.write_changes(&mut out, &next, width);
            let mut pencil = Pencil { out: &mut out, row, col, width: width.into() };
            let to = committed_rows + rows - below_rows;
            pencil.go(to, cursor.map_or(0, |cursor| cursor.column));
            let lines = next.split_off(committed_texts.len());
            (done, lines)
        };
        // NOTE: a frame that changed nothing on the screen, such as a block that commits as
        // it stands, writes nothing at all.
        if out.len() == BEGIN_SYNC.len() {
            out.clear();
        } else {
            out.push_str(END_SYNC);
        }
        #[cfg(test)]
        {
            let tail_lines = tail.text.matches('\n').count();
            let status_index = (committed_texts.len() + texts.len())
                .checked_sub(first + tail_lines + 1)
                .filter(|_| !status.is_empty());
            crate::testing::probe::zone(crate::testing::probe::Zone {
                empty: false,
                status_only: committed.is_empty()
                    && !done.more
                    && status_index.is_some_and(|index| done.in_place == [index]),
                committed_rows,
                erased_rows: done.erased,
                live_rows: rows,
                hidden_rows: rows_of(&live, width, method).saturating_sub(rows),
            });
        }
        #[cfg(not(test))]
        drop(done);
        self.lines = lines
            .into_iter()
            .map(|line| Line {
                text: line.text.to_owned(),
                layout: line.layout,
                hidden: line.hidden,
            })
            .collect();
        self.first = first;
        self.shown = shown;
        self.cursor = cursor;
        self.below = below;
        self.rows = rows;
        self.width = width;
        self.size = size;
        out
    }

    /// Lays out the live lines `texts` from the last: the lines that fit on the screen
    /// whole, and above them the last rows of the line before them. Gives the lines of
    /// the live zone, the index of the first in `texts`, and the rows they take.
    fn lay_out<'a>(
        &self,
        texts: &[&'a str],
        committed: usize,
        width: u16,
        reuse: bool,
        size: Size,
    ) -> (Vec<Next<'a>>, usize, usize) {
        let max = max_rows(size);
        let mut lines = Vec::new();
        let mut rows = 0;
        let mut first = texts.len();
        for (index, text) in texts.iter().enumerate().rev() {
            // NOTE: the line in the same place on the screen, when the clip leaves out as
            // many lines as before.
            let old = (reuse && index + committed >= self.first)
                .then(|| self.lines.get(index + committed - self.first))
                .flatten();
            let mut line = lay(text, old, width, self.method);
            let line_rows = line.layout.rows();
            if let Some(max) = max
                && rows + line_rows > max
            {
                let left = max - rows;
                if left > 0 {
                    line.hidden = line_rows - left;
                    lines.push(line);
                    first = index;
                    rows = max;
                }
                break;
            }
            rows += line_rows;
            first = index;
            lines.push(line);
        }
        lines.reverse();
        (lines, first, rows)
    }

    /// The frame that erases the whole old live zone and writes `committed` and `shown`.
    fn write_whole(&self, out: &mut String, committed: &str, shown: &str, width: u16) {
        // The rows from the cursor down to the row below the live zone.
        let parked = rows_of(&self.below, width, self.method);
        let old_rows =
            if width == self.width { self.rows } else { rows_of(&self.shown, width, self.method) };
        let up = old_rows.saturating_sub(parked);
        if up > 0 {
            let _ = write!(out, "\r\x1b[{up}A\x1b[J");
        } else if old_rows > 0 {
            out.push_str("\r\x1b[J");
        }
        out.push_str(committed);
        out.push_str(shown);
    }

    /// Writes what changed between the lines on the screen and `next`, the committed
    /// lines and the live lines of the frame. Gives what it did, and the row and the
    /// column where it left the cursor.
    fn write_changes(
        &self,
        out: &mut String,
        next: &[Next<'_>],
        width: u16,
    ) -> (Done, usize, Option<usize>) {
        let parked = rows_of(&self.below, width, self.method);
        let start_row = self.rows - parked.min(self.rows);
        let start_col = self.cursor.is_none().then_some(0);
        let mut pencil = Pencil { out, row: start_row, col: start_col, width: width.into() };
        let mut done = Done::default();
        // The rows of the old live zone that are on the screen.
        let mut old_rows = self.rows;
        // Rows deleted at the top of the live zone.
        let mut deleted = 0;
        // The row where the line `index` starts, on the screen and in the frame.
        let mut row = 0;
        for (index, line) in next.iter().enumerate() {
            let Some(old) = self.lines.get(index) else {
                rewrite(&mut pencil, &mut done, next, index, (row, 0), None, old_rows);
                return (done, pencil.row, pencil.col);
            };
            let mut old_hidden = old.hidden + if index == 0 { deleted } else { 0 };
            if line.same && line.hidden == old_hidden {
                row += line.shown_rows();
                continue;
            }
            let differs = line.spot.as_ref().map_or(line.layout.rows(), |spot| spot.row);
            if line.hidden != old_hidden {
                // A line taller than the live zone grew: its rows move up.
                let up = line.hidden.wrapping_sub(old_hidden);
                if index == 0
                    && line.hidden > old_hidden
                    && differs >= line.hidden
                    && up < old.layout.rows() - old_hidden
                {
                    pencil.go(0, 0);
                    let _ = write!(pencil.out, "\x1b[{up}M");
                    deleted = up;
                    old_rows -= up;
                    old_hidden = line.hidden;
                    done.erased += up;
                    done.more = true;
                } else {
                    rewrite(&mut pencil, &mut done, next, index, (row, 0), None, old_rows);
                    return (done, pencil.row, pencil.col);
                }
                if line.same {
                    row += line.shown_rows();
                    continue;
                }
            }
            let Some(spot) = &line.spot else {
                unreachable!("a line that changed has a spot");
            };
            if spot.row < line.hidden {
                rewrite(&mut pencil, &mut done, next, index, (row, 0), None, old_rows);
                return (done, pencil.row, pencil.col);
            }
            let at = (row + spot.row - line.hidden, spot.col);
            // NOTE: a line that changed from its first cell writes as much in place as in
            // a write to the end; in place pays only when the lines after it stay.
            let rest_stays = spot.row > 0
                || spot.col > 0
                || next.get(index + 1).map_or(self.lines.len() == index + 1, |next| next.same);
            if old.layout.rows() - old_hidden == line.shown_rows()
                && spot.row + 1 == line.layout.rows()
                && rest_stays
            {
                // Only the last row changed: write it from the first cell that changed.
                let rest = strip(&line.text[spot.at..]);
                let erase = old.layout.end > line.layout.end;
                if spot.col < pencil.width && (!rest.is_empty() || erase) {
                    pencil.go(at.0, at.1);
                    spot.pen.push_to(pencil.out);
                    pencil.out.push_str(rest);
                    pencil.after(line.layout.end);
                    if erase {
                        pencil.out.push_str("\x1b[K");
                    }
                    done.erased += 1;
                    done.in_place.push(index);
                }
                row += line.shown_rows();
                continue;
            }
            if spot.col >= pencil.width {
                // No cell changed: the rest is escapes, and the next line is new.
                let next_row = row + line.shown_rows();
                rewrite(&mut pencil, &mut done, next, index + 1, (next_row, 0), None, old_rows);
            } else {
                rewrite(&mut pencil, &mut done, next, index, at, Some(spot), old_rows);
            }
            return (done, pencil.row, pencil.col);
        }
        if self.lines.len() > next.len() && row < old_rows {
            pencil.go(row, 0);
            pencil.out.push_str("\x1b[J");
            done.erased += old_rows - row;
            done.more = true;
        }
        (done, pencil.row, pencil.col)
    }
}

/// Writes the lines of `next` from `index` on, from the cell `at`: from `spot` in the
/// line `index`, or from its first row on the screen. Erases first what the old live
/// zone, `old_rows` rows, holds from there.
fn rewrite(
    pencil: &mut Pencil<'_>,
    done: &mut Done,
    next: &[Next<'_>],
    index: usize,
    at: (usize, usize),
    spot: Option<&Spot>,
    old_rows: usize,
) {
    done.more = true;
    let Some(line) = next.get(index) else {
        // Nothing comes after: only the erase.
        if at.0 < old_rows {
            pencil.go(at.0, 0);
            pencil.out.push_str("\x1b[J");
            done.erased += old_rows - at.0;
        }
        return;
    };
    pencil.go(at.0, at.1);
    if at.0 < old_rows {
        pencil.out.push_str("\x1b[J");
        done.erased += old_rows - at.0;
    }
    let mut row = at.0;
    match spot {
        Some(spot) => {
            spot.pen.push_to(pencil.out);
            pencil.out.push_str(&line.text[spot.at..]);
            row += line.layout.rows() - spot.row;
        }
        None => {
            line.push_shown(pencil.out);
            row += line.shown_rows();
        }
    }
    for line in &next[index + 1..] {
        line.push_shown(pencil.out);
        row += line.shown_rows();
    }
    pencil.row = row;
    pencil.col = Some(0);
}

/// `text` laid out at `width`, against `old`, the line in the same place on the screen.
fn lay<'a>(text: &'a str, old: Option<&Line>, width: u16, method: WidthMethod) -> Next<'a> {
    let line = strip(text);
    match old {
        Some(old) if old.text == text => {
            Next { text, layout: old.layout.clone(), hidden: 0, same: true, spot: None }
        }
        Some(old) => {
            let same = old.text.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
            let (layout, spot) = relayout(line, &old.layout, same, width.into(), method);
            Next { text, layout, hidden: 0, same: false, spot: Some(spot) }
        }
        None => {
            let layout = layout(line, width.into(), method);
            let spot = Spot { row: 0, col: 0, at: 0, pen: Default::default() };
            Next { text, layout, hidden: 0, same: false, spot: Some(spot) }
        }
    }
}

/// `text` without its newline.
fn strip(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

/// The last lines of the live zone, below the status row: the input row of a turn,
/// each line with its newline, and where the cursor waits in them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tail {
    pub(crate) text: String,
    pub(crate) cursor: Option<Cursor>,
}

/// The width that rendering and row counting use for a terminal of `size`.
pub(crate) fn effective_width(size: Size) -> u16 {
    if size.cols == 0 { FALLBACK_WIDTH } else { size.cols }
}

/// The most rows the live zone may take: one less than the screen, so it never fills
/// it. `None` when the height is unknown.
fn max_rows(size: Size) -> Option<usize> {
    (size.rows > 0).then(|| usize::from(size.rows) - 1)
}

/// True when `rows` rows fit in the live zone of a screen of `size`.
pub(crate) fn fits(rows: usize, size: Size) -> bool {
    max_rows(size).is_none_or(|max| rows <= max)
}

/// The rows `text` takes on a terminal `width` columns wide that counts widths by
/// `method`: each line at least one, and one more for every time it wraps.
pub(crate) fn rows_of(text: &str, width: u16, method: WidthMethod) -> usize {
    text.split_inclusive('\n')
        .map(|line| display_rows(strip(line), usize::from(width), method))
        .sum()
}

#[cfg(test)]
mod tests;
