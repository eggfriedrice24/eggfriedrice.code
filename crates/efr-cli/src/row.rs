//! The line of the input row: what the user types while a turn runs, to steer it, to
//! queue a prompt behind it, to interrupt it or to take back a queued prompt.
//!
//! The key thread hands over single bytes and a lone Esc (`crate::keys`), so this
//! module decodes the escape sequences of the keys and edits the line itself:
//!
//! - Printable text is added at the cursor, UTF-8 whole: the bytes of a character wait
//!   until it is complete.
//! - Backspace and Delete remove one grapheme cluster; Left and Right move over one.
//! - Home and End (also Ctrl+A and Ctrl+E) go to the start and the end of the line the
//!   cursor is on; Ctrl+U removes the text from the start of that line to the cursor,
//!   Ctrl+K from the cursor to its end, and Ctrl+W the word before the cursor (up to a
//!   blank). Alt+B and Alt+F go one word back and forward (letters and digits).
//! - Ctrl+J and Alt+Enter add a newline. A bracketed paste (`CSI 200 ~` to
//!   `CSI 201 ~`) is added whole, with its newlines: a newline in it never sends.
//! - Enter steers, Tab queues, Esc interrupts and Alt+Up takes back the newest prompt
//!   queued from here. Those keys only say so ([`Action`]); the follow loop acts.
//!
//! Other control characters and escape sequences change nothing. The line holds no
//! secret: it is shown as it is typed, and it goes to the daemon as a steer or a
//! prompt.

use efr_render::{WidthMethod, text_width};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation as _};

use crate::keys::{ESC, Key};

/// The most bytes the line holds; a longer paste is cut at a character.
pub(crate) const MAX_BYTES: usize = 256 * 1024;

/// What ends a bracketed paste.
const PASTE_END: &[u8] = b"\x1b[201~";

/// The most bytes of parameters an escape sequence may have before it counts as noise.
const MAX_PARAMS: usize = 16;

/// What a key asks the follow loop to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Nothing changed.
    None,
    /// The text or the cursor changed: draw the row again.
    Edited,
    /// Enter: send the text as a steer of the running turn.
    Steer,
    /// Tab: send the text as a prompt queued behind the running turn.
    Queue,
    /// Esc: interrupt the turn.
    Interrupt,
    /// Alt+Up: take back the newest prompt queued from here.
    Withdraw,
}

/// Where the decoder is in an escape sequence or a paste.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum State {
    /// Not in one.
    #[default]
    Plain,
    /// After an escape byte; `alt` after two, as some terminals send Alt with an arrow.
    Escape { alt: bool },
    /// In a CSI sequence (`ESC [`), with its parameter bytes so far.
    Csi { alt: bool, params: Vec<u8> },
    /// After `ESC O`, which one more byte ends.
    Ss3 { alt: bool },
    /// In a bracketed paste: the text so far, and how many bytes of [`PASTE_END`]
    /// came last.
    Paste { text: Vec<u8>, matched: usize },
}

/// The text of the input row and its cursor.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowLine {
    text: String,
    /// A byte offset into `text`, always at the start of a grapheme cluster or at the
    /// end.
    cursor: usize,
    state: State,
    /// The bytes of a UTF-8 character that has not arrived whole.
    utf8: Vec<u8>,
}

/// The rows of the line at one width, and where the cursor is on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    /// The rows, without a newline; each one fits the width it was laid out for.
    pub(crate) rows: Vec<String>,
    /// The row of the cursor.
    pub(crate) cursor_row: usize,
    /// The column of the cursor in its row.
    pub(crate) cursor_col: usize,
}

impl RowLine {
    /// The text typed so far.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The byte offset of the cursor in the text.
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// True when the line has no text.
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// True when the line has no text that is not blank: Enter and Tab send nothing.
    pub(crate) fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Takes the text and leaves the line empty.
    pub(crate) fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    /// Removes the text; true when there was any.
    pub(crate) fn clear(&mut self) -> bool {
        let had = !self.text.is_empty();
        self.text.clear();
        self.cursor = 0;
        had
    }

    /// Puts `text` back after the text that the line holds, on a line of its own, and
    /// the cursor at the end: a prompt taken back or a steer that was not sent.
    pub(crate) fn append(&mut self, text: &str) {
        if text.is_empty() || self.text.len() >= MAX_BYTES {
            return;
        }
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        let room = MAX_BYTES.saturating_sub(self.text.len());
        self.text.push_str(cut_at(text, room));
        self.cursor = self.text.len();
    }

    /// True while a bracketed paste has started and not ended.
    pub(crate) fn pasting(&self) -> bool {
        matches!(self.state, State::Paste { .. })
    }

    /// Drops an escape sequence or a character that has not arrived whole: the keys
    /// went elsewhere meanwhile, so the rest of it will not come. A paste goes on.
    pub(crate) fn drop_partial(&mut self) {
        if !self.pasting() {
            self.state = State::Plain;
        }
        self.utf8.clear();
    }

    /// Ends a bracketed paste whose end did not come: the text so far is added.
    pub(crate) fn end_paste(&mut self) {
        if let State::Paste { mut text, matched } = std::mem::take(&mut self.state) {
            text.extend_from_slice(&PASTE_END[..matched]);
            let pasted = pasted_text(&text);
            self.insert(&pasted);
        }
    }

    /// Takes one key.
    pub(crate) fn key(&mut self, key: Key) -> Action {
        let byte = match (key, &self.state) {
            // A paste may hold an escape byte that nothing followed in time.
            (Key::Esc, State::Paste { .. }) => ESC,
            (Key::Esc, _) => {
                self.state = State::Plain;
                self.utf8.clear();
                return Action::Interrupt;
            }
            (Key::Byte(byte), _) => byte,
        };
        match std::mem::take(&mut self.state) {
            State::Plain => self.plain(byte),
            State::Escape { alt } => self.escape(byte, alt),
            State::Csi { alt, mut params } => {
                match byte {
                    0x20..=0x3f if params.len() < MAX_PARAMS => {
                        params.push(byte);
                        self.state = State::Csi { alt, params };
                    }
                    0x40..=0x7e => return self.csi(byte, alt, &params),
                    // Noise: the sequence is dropped.
                    _ => {}
                }
                Action::None
            }
            State::Ss3 { alt } => self.ss3(byte, alt),
            State::Paste { text, matched } => self.paste(byte, text, matched),
        }
    }

    fn plain(&mut self, byte: u8) -> Action {
        if !self.utf8.is_empty() || byte >= 0x80 {
            return self.utf8(byte);
        }
        match byte {
            b'\r' => Action::Steer,
            b'\t' => Action::Queue,
            // Ctrl+J.
            b'\n' => self.insert("\n"),
            0x7f | 0x08 => self.backspace(),
            ESC => {
                self.state = State::Escape { alt: false };
                Action::None
            }
            // Ctrl+A, Ctrl+E.
            0x01 => self.move_to(self.line_start()),
            0x05 => self.move_to(self.line_end()),
            // Ctrl+B, Ctrl+F.
            0x02 => self.move_to(self.previous()),
            0x06 => self.move_to(self.next()),
            // Ctrl+D.
            0x04 => self.delete(),
            // Ctrl+U, Ctrl+K, Ctrl+W.
            0x15 => self.remove(self.line_start(), self.cursor),
            0x0b => self.remove(self.cursor, self.line_end()),
            0x17 => self.remove(self.blank_word_start(), self.cursor),
            0x00..=0x1f => Action::None,
            _ => self.insert(char::from(byte).encode_utf8(&mut [0; 4])),
        }
    }

    fn escape(&mut self, byte: u8, alt: bool) -> Action {
        match byte {
            b'[' => self.state = State::Csi { alt, params: Vec::new() },
            b'O' => self.state = State::Ss3 { alt },
            ESC if !alt => self.state = State::Escape { alt: true },
            b'b' | b'B' => return self.move_to(self.word_start()),
            b'f' | b'F' => return self.move_to(self.word_end()),
            // Alt+Enter.
            b'\r' => return self.insert("\n"),
            // Alt+Backspace.
            0x7f | 0x08 => return self.remove(self.word_start(), self.cursor),
            // Any other Alt and a key: nothing. A control character means what it
            // always means.
            0x20..=0x7e => {}
            _ => return self.plain(byte),
        }
        Action::None
    }

    fn csi(&mut self, last: u8, alt: bool, params: &[u8]) -> Action {
        let params = std::str::from_utf8(params).unwrap_or("");
        let mut fields = params.split(';');
        let first = fields.next().unwrap_or("");
        // xterm's modifier: 1 plus 2 for Alt, 4 for Ctrl (and 1 for Shift).
        let modifier = fields.next().and_then(|field| field.parse::<u8>().ok()).unwrap_or(1);
        let alt = alt || matches!(modifier, 3 | 4 | 7 | 8);
        let word = alt || matches!(modifier, 5 | 6);
        match (last, first) {
            (b'A', _) if alt => Action::Withdraw,
            (b'C', _) if word => self.move_to(self.word_end()),
            (b'D', _) if word => self.move_to(self.word_start()),
            (b'C', _) => self.move_to(self.next()),
            (b'D', _) => self.move_to(self.previous()),
            (b'H', _) | (b'~', "1" | "7") => self.move_to(self.line_start()),
            (b'F', _) | (b'~', "4" | "8") => self.move_to(self.line_end()),
            (b'~', "3") => self.delete(),
            (b'~', "200") => {
                self.state = State::Paste { text: Vec::new(), matched: 0 };
                Action::None
            }
            _ => Action::None,
        }
    }

    fn ss3(&mut self, byte: u8, alt: bool) -> Action {
        match byte {
            b'A' if alt => Action::Withdraw,
            b'C' if alt => self.move_to(self.word_end()),
            b'D' if alt => self.move_to(self.word_start()),
            b'C' => self.move_to(self.next()),
            b'D' => self.move_to(self.previous()),
            b'H' => self.move_to(self.line_start()),
            b'F' => self.move_to(self.line_end()),
            _ => Action::None,
        }
    }

    /// One byte of a bracketed paste. The bytes that may start its end wait in
    /// `matched` until they do or do not.
    fn paste(&mut self, byte: u8, mut text: Vec<u8>, mut matched: usize) -> Action {
        if PASTE_END.get(matched) == Some(&byte) {
            matched += 1;
            if matched == PASTE_END.len() {
                let pasted = pasted_text(&text);
                return self.insert(&pasted);
            }
            self.state = State::Paste { text, matched };
            return Action::None;
        }
        let mut push = |bytes: &[u8]| {
            let room = MAX_BYTES.saturating_sub(text.len());
            text.extend_from_slice(&bytes[..bytes.len().min(room)]);
        };
        push(&PASTE_END[..matched]);
        if PASTE_END.first() == Some(&byte) {
            matched = 1;
        } else {
            matched = 0;
            push(&[byte]);
        }
        self.state = State::Paste { text, matched };
        Action::None
    }

    /// A byte of a multibyte character: kept until the character is whole.
    fn utf8(&mut self, byte: u8) -> Action {
        let continuation = byte & 0xc0 == 0x80;
        if !self.utf8.is_empty() && !continuation {
            // The character before was cut short; the new byte starts afresh.
            self.utf8.clear();
            return self.plain(byte);
        }
        let lead = self.utf8.first().copied().unwrap_or(byte);
        let Some(expected) = utf8_len(lead) else {
            self.utf8.clear();
            return Action::None;
        };
        self.utf8.push(byte);
        if self.utf8.len() < expected {
            return Action::None;
        }
        let bytes = std::mem::take(&mut self.utf8);
        match std::str::from_utf8(&bytes) {
            Ok(text) if !text.chars().any(char::is_control) => self.insert(text),
            _ => Action::None,
        }
    }

    /// Adds `text` at the cursor, as much as fits.
    fn insert(&mut self, text: &str) -> Action {
        let room = MAX_BYTES.saturating_sub(self.text.len());
        let text = cut_at(text, room);
        if text.is_empty() {
            return Action::None;
        }
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        Action::Edited
    }

    fn backspace(&mut self) -> Action {
        self.remove(self.previous(), self.cursor)
    }

    fn delete(&mut self) -> Action {
        self.remove(self.cursor, self.next())
    }

    /// Removes the text from `start` to `end` and puts the cursor at `start`.
    fn remove(&mut self, start: usize, end: usize) -> Action {
        if start >= end {
            return Action::None;
        }
        self.text.replace_range(start..end, "");
        self.cursor = start;
        Action::Edited
    }

    fn move_to(&mut self, at: usize) -> Action {
        if at == self.cursor {
            return Action::None;
        }
        self.cursor = at;
        Action::Edited
    }

    /// The start of the grapheme cluster before the cursor.
    fn previous(&self) -> usize {
        let mut cursor = GraphemeCursor::new(self.cursor, self.text.len(), true);
        cursor.prev_boundary(&self.text, 0).ok().flatten().unwrap_or(0)
    }

    /// The end of the grapheme cluster after the cursor.
    fn next(&self) -> usize {
        let mut cursor = GraphemeCursor::new(self.cursor, self.text.len(), true);
        cursor.next_boundary(&self.text, 0).ok().flatten().unwrap_or(self.text.len())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |at| at + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |at| self.cursor + at)
    }

    /// The start of the word before the cursor, where a word is letters and digits.
    fn word_start(&self) -> usize {
        back_over(&self.text[..self.cursor], |piece| !is_word(piece), is_word)
    }

    /// The end of the word after the cursor, where a word is letters and digits.
    fn word_end(&self) -> usize {
        let rest = &self.text[self.cursor..];
        let mut at = 0;
        let mut in_word = false;
        for (offset, piece) in rest.grapheme_indices(true) {
            if is_word(piece) {
                in_word = true;
            } else if in_word {
                return self.cursor + offset;
            }
            at = offset + piece.len();
        }
        self.cursor + at
    }

    /// The start of the word before the cursor, where a word is anything but blanks.
    fn blank_word_start(&self) -> usize {
        back_over(&self.text[..self.cursor], is_blank, |piece| !is_blank(piece))
    }

    /// The rows of the line when a row holds `columns` columns of text, on a terminal
    /// that counts widths by `method`, and where the cursor is. Each line of the text
    /// starts a row, and a line longer than a row goes on in the next one, never inside
    /// a grapheme cluster. A tab shows as a blank and another control character as its
    /// stand-in, so nothing typed or pasted can drive the terminal.
    pub(crate) fn layout(&self, columns: usize, method: WidthMethod) -> Layout {
        let columns = columns.max(1);
        let mut rows = vec![String::new()];
        let mut used = 0;
        let mut cursor = None;
        for (offset, piece) in self.text.grapheme_indices(true) {
            if offset == self.cursor {
                cursor = Some((rows.len() - 1, used));
            }
            if piece == "\n" || piece == "\r\n" {
                rows.push(String::new());
                used = 0;
                continue;
            }
            let shown = shown_piece(piece);
            let width = text_width(&shown, method);
            if used > 0 && used + width > columns {
                rows.push(String::new());
                used = 0;
                if offset == self.cursor {
                    cursor = Some((rows.len() - 1, 0));
                }
            }
            if let Some(row) = rows.last_mut() {
                row.push_str(&shown);
            }
            used += width;
        }
        let (cursor_row, cursor_col) = cursor.unwrap_or((rows.len() - 1, used));
        Layout { rows, cursor_row, cursor_col }
    }
}

/// How a grapheme cluster of the line shows: a tab as a blank, a control or format
/// character as its stand-in, except the zero width joiner inside an emoji.
fn shown_piece(piece: &str) -> String {
    let mut shown = String::new();
    for c in piece.chars() {
        match c {
            '\t' => shown.push(' '),
            ZWJ => shown.push(ZWJ),
            c => shown.push_str(&crate::format::one_line(c.encode_utf8(&mut [0; 4]))),
        }
    }
    shown
}

/// The zero width joiner, which makes one emoji of several.
const ZWJ: char = '\u{200d}';

/// The start of the run that `inside` matches before the end of `text`, after the
/// pieces that `skip` matches at the end are passed over.
fn back_over(text: &str, skip: impl Fn(&str) -> bool, inside: impl Fn(&str) -> bool) -> usize {
    let mut at = text.len();
    let mut skipping = true;
    for (offset, piece) in text.grapheme_indices(true).rev() {
        if skipping && skip(piece) {
            at = offset;
            continue;
        }
        skipping = false;
        if !inside(piece) {
            break;
        }
        at = offset;
    }
    at
}

/// True for a grapheme cluster that is part of a word: it starts with a letter or a
/// digit.
fn is_word(piece: &str) -> bool {
    piece.chars().next().is_some_and(char::is_alphanumeric)
}

fn is_blank(piece: &str) -> bool {
    piece.chars().all(char::is_whitespace)
}

/// The text of a paste: carriage returns as newlines, the other control characters
/// left out except tabs.
fn pasted_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n").replace('\r', "\n");
    text.chars().filter(|c| matches!(c, '\n' | '\t') || !c.is_control()).collect()
}

/// The longest start of `text` of at most `bytes` bytes that ends on a character.
fn cut_at(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The length of a UTF-8 character that starts with `lead`; `None` for a byte that
/// cannot start one.
fn utf8_len(lead: u8) -> Option<usize> {
    match lead {
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
