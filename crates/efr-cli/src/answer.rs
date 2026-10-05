//! The line that the user types for a running command that waits for input, such as the
//! password that `sudo` asks for.
//!
//! The key thread hands over single bytes in non-canonical mode, so this module does
//! the line editing that the terminal would do in canonical mode: printable text is
//! added, Backspace removes one character, Ctrl+U clears the line and Enter sends it.
//! Escape sequences (arrow keys, function keys) are dropped whole, and other control
//! characters are ignored, because `input.respond` takes one line without them.
//!
//! The text can be a password. It never shows in `Debug`, and it is overwritten with
//! zeros when it is cleared, sent or dropped. Its buffer is allocated at the full size
//! of an answer up front, so it never moves while it grows: a move would leave a copy
//! in freed memory that nothing zeroes.

use std::fmt;

use efr_protocol::SecretText;
use zeroize::{Zeroize as _, Zeroizing};

/// The longest answer in bytes, the limit of `input.respond`.
pub(crate) const MAX_ANSWER_BYTES: usize = 1024;

/// What one key did to the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edit {
    /// Nothing changed: an ignored key, part of an escape sequence or of a character.
    Unchanged,
    /// The text changed.
    Changed,
    /// Enter: the line is ready to send.
    Submit,
}

/// Where the line is in an escape sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    /// Not in one.
    None,
    /// After ESC.
    Start,
    /// In a CSI sequence (`ESC [`), which ends with a byte from 0x40 to 0x7e.
    Csi,
    /// After `ESC O`, which one more byte ends.
    Ss3,
}

/// A line of text typed one byte at a time.
pub(crate) struct AnswerLine {
    text: Zeroizing<String>,
    /// The bytes of a UTF-8 character that has not arrived whole.
    pending: Zeroizing<[u8; 4]>,
    pending_len: usize,
    escape: Escape,
}

impl fmt::Debug for AnswerLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnswerLine").field("bytes", &self.text.len()).finish_non_exhaustive()
    }
}

impl Default for AnswerLine {
    fn default() -> Self {
        AnswerLine::new()
    }
}

impl AnswerLine {
    /// An empty line.
    pub(crate) fn new() -> AnswerLine {
        AnswerLine {
            text: Zeroizing::new(String::with_capacity(MAX_ANSWER_BYTES)),
            pending: Zeroizing::new([0; 4]),
            pending_len: 0,
            escape: Escape::None,
        }
    }

    /// The text typed so far. Only a visible answer is ever shown.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Takes the text to send and leaves the line empty. The allocation moves into the
    /// [`SecretText`], which zeroes it when it is dropped.
    pub(crate) fn take(&mut self) -> SecretText {
        self.drop_pending();
        self.escape = Escape::None;
        let text = std::mem::replace(&mut *self.text, String::with_capacity(MAX_ANSWER_BYTES));
        SecretText::new(text)
    }

    /// Takes one key.
    pub(crate) fn key(&mut self, byte: u8) -> Edit {
        match self.escape {
            Escape::None => {}
            Escape::Start => {
                match byte {
                    b'[' => self.escape = Escape::Csi,
                    b'O' => self.escape = Escape::Ss3,
                    // Alt and a key: the key goes with the ESC.
                    0x20..=0x7e => self.escape = Escape::None,
                    // A lone ESC: the key after it means what it always means.
                    _ => {
                        self.escape = Escape::None;
                        return self.key(byte);
                    }
                }
                return Edit::Unchanged;
            }
            Escape::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.escape = Escape::None;
                }
                return Edit::Unchanged;
            }
            Escape::Ss3 => {
                self.escape = Escape::None;
                return Edit::Unchanged;
            }
        }
        if self.pending_len > 0 || byte >= 0x80 {
            return self.utf8(byte);
        }
        match byte {
            b'\r' | b'\n' => Edit::Submit,
            0x7f | 0x08 => self.backspace(),
            // Ctrl+U, the terminal's line kill.
            0x15 => self.clear(),
            0x1b => {
                self.escape = Escape::Start;
                Edit::Unchanged
            }
            0x00..=0x1f => Edit::Unchanged,
            _ => self.push_char(char::from(byte)),
        }
    }

    /// A byte of a multibyte character: kept until the character is whole.
    fn utf8(&mut self, byte: u8) -> Edit {
        let continuation = byte & 0xc0 == 0x80;
        if self.pending_len > 0 && !continuation {
            // The character before was cut short; the new byte starts afresh.
            self.drop_pending();
            return self.key(byte);
        }
        let lead = if self.pending_len == 0 { byte } else { self.pending[0] };
        let Some(expected) = utf8_len(lead) else {
            return Edit::Unchanged;
        };
        self.pending[self.pending_len] = byte;
        self.pending_len += 1;
        if self.pending_len < expected {
            return Edit::Unchanged;
        }
        let edit = match std::str::from_utf8(&self.pending[..self.pending_len]) {
            Ok(text) => text.chars().next().map_or(Edit::Unchanged, |c| self.push_char(c)),
            Err(_) => Edit::Unchanged,
        };
        self.drop_pending();
        edit
    }

    fn push_char(&mut self, c: char) -> Edit {
        if c.is_control() || self.text.len() + c.len_utf8() > MAX_ANSWER_BYTES {
            return Edit::Unchanged;
        }
        self.text.push(c);
        Edit::Changed
    }

    fn backspace(&mut self) -> Edit {
        // `pop` takes a whole character, so a multibyte one is never split. Its bytes
        // stay in the spare capacity until the next clear, take or drop zeroes it.
        if self.text.pop().is_some() { Edit::Changed } else { Edit::Unchanged }
    }

    fn clear(&mut self) -> Edit {
        if self.text.is_empty() {
            return Edit::Unchanged;
        }
        // Zeroes the whole capacity and keeps it, so the buffer never moves.
        self.text.zeroize();
        Edit::Changed
    }

    fn drop_pending(&mut self) {
        self.pending.zeroize();
        self.pending_len = 0;
    }
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
