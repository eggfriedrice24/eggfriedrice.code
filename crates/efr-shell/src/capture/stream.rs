//! The byte cleaner of [`clean`](super::clean) over output that comes in chunks, and
//! its grammar alone, which tells whether a stream ends inside an escape sequence.

use std::borrow::Cow;

use super::{escape_end, push_text};

/// The most bytes of an unfinished line that [`StreamClean`] holds before it gives
/// them out as they are.
pub(crate) const STREAM_LINE_LIMIT: usize = 64 * 1024;

/// The most bytes of an unfinished escape sequence that [`StreamClean`] holds. A longer
/// one keeps only what tells its kind and how it can end.
const STREAM_CARRY_LIMIT: usize = 4096;

/// The byte cleaner of [`clean`](super::clean) over output that comes in chunks, such
/// as the output of a command that a background job adopted. It gives the same text as
/// [`clean`](super::clean) on the whole output:
///
/// - an escape sequence split across two chunks is read whole: the chunk keeps its
///   start until the next chunk ends it;
/// - a line is given out only once it ends with `\n`, so a bare `\r` can still rewrite
///   it and a progress bar leaves its last state; a line that grows past
///   [`STREAM_LINE_LIMIT`] without an end is given out as it is, and a `\r` after that
///   rewrites only the rest;
/// - a UTF-8 character split across two chunks stays whole, because a line is given
///   out at a `\n` or at a character boundary.
#[derive(Debug, Default)]
pub(crate) struct StreamClean {
    /// The start of an escape sequence that the last chunk ended inside.
    carry: Vec<u8>,
    /// The current line, not given out yet.
    line: Vec<u8>,
    /// A bare `\r` came: the next text replaces the line.
    carriage_return: bool,
}

impl StreamClean {
    /// Takes the next chunk of the output and returns the text that it completes.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> String {
        let bytes = after_carry(&mut self.carry, chunk);
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let byte = bytes[at];
            match byte {
                0x1b => match escape_end(&bytes, at) {
                    Some(end) => {
                        at = end;
                        continue;
                    }
                    None => {
                        self.carry = carry_of(&bytes[at..]);
                        break;
                    }
                },
                b'\n' => {
                    self.line.push(b'\n');
                    out.append(&mut self.line);
                    self.carriage_return = false;
                }
                b'\r' => self.carriage_return = true,
                0x08 => {
                    // A whole UTF-8 character goes: its continuation bytes, then its lead.
                    while let Some(byte) = self.line.pop() {
                        if byte & 0xc0 != 0x80 {
                            break;
                        }
                    }
                }
                b'\t' => push_text(&mut self.line, 0, &mut self.carriage_return, byte),
                0x00..=0x1f | 0x7f => {}
                _ => push_text(&mut self.line, 0, &mut self.carriage_return, byte),
            }
            at += 1;
        }
        if self.line.len() > STREAM_LINE_LIMIT {
            let whole = self.line.len() - unfinished_char(&self.line);
            out.extend(self.line.drain(..whole));
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Drops the start of an escape sequence that the last chunk ended inside: the
    /// bytes that came next were no part of it, such as at a shell mark that the
    /// session took out of the stream.
    pub(crate) fn mark(&mut self) {
        self.carry.clear();
    }

    /// Ends the current line where it is, for bytes that are missing after it, and
    /// returns its text with a line feed; nothing when the line is empty.
    pub(crate) fn break_line(&mut self) -> String {
        self.carry.clear();
        self.carriage_return = false;
        if self.line.is_empty() {
            return String::new();
        }
        self.line.push(b'\n');
        let line = std::mem::take(&mut self.line);
        String::from_utf8_lossy(&line).into_owned()
    }

    /// The output ended: returns the rest of the last line. An escape sequence that
    /// never ended is dropped, as [`clean`](super::clean) drops it.
    pub(crate) fn finish(&mut self) -> String {
        self.carry.clear();
        self.carriage_return = false;
        let line = std::mem::take(&mut self.line);
        String::from_utf8_lossy(&line).into_owned()
    }
}

/// Where the bytes of a stream stand in the grammar of [`clean`](super::clean): outside
/// of any escape sequence, or inside one. It reads the bytes as `clean` does, but makes
/// no text.
#[derive(Debug, Default)]
pub(crate) struct OpenSequence {
    /// What [`carry_of`] keeps of the escape sequence that the bytes so far end inside;
    /// empty when they end outside of any sequence.
    carry: Vec<u8>,
}

impl OpenSequence {
    /// Reads the next bytes of the stream.
    pub(crate) fn feed(&mut self, chunk: &[u8]) {
        let bytes = after_carry(&mut self.carry, chunk);
        let mut at = 0;
        while let Some(found) = bytes[at..].iter().position(|&byte| byte == 0x1b) {
            let start = at + found;
            match escape_end(&bytes, start) {
                Some(end) => at = end,
                None => {
                    self.carry = carry_of(&bytes[start..]);
                    return;
                }
            }
        }
    }

    /// How many bytes at the start of `next`, the bytes that come after the bytes fed,
    /// are still a part of the open sequence: none when no sequence is open, all of
    /// `next` when the sequence does not end in it.
    pub(crate) fn rest_in(&self, next: &[u8]) -> usize {
        if self.carry.is_empty() {
            return 0;
        }
        let mut joined = self.carry.clone();
        joined.extend_from_slice(next);
        escape_end(&joined, 0).map_or(next.len(), |end| end - self.carry.len())
    }
}

/// `carry`, the start of an escape sequence that the last chunk ended inside, and then
/// `chunk`, as one slice; `carry` is empty after it. Only a pending carry costs a copy.
fn after_carry<'a>(carry: &mut Vec<u8>, chunk: &'a [u8]) -> Cow<'a, [u8]> {
    if carry.is_empty() {
        return Cow::Borrowed(chunk);
    }
    let mut joined = std::mem::take(carry);
    joined.extend_from_slice(chunk);
    Cow::Owned(joined)
}

/// What [`StreamClean`] and [`OpenSequence`] keep of `sequence`, an escape sequence that
/// has not ended yet: all of it, or, past [`STREAM_CARRY_LIMIT`], its `ESC` and kind and
/// a last `ESC`, which may start its `ST`. The body of a sequence never reaches the
/// text.
fn carry_of(sequence: &[u8]) -> Vec<u8> {
    if sequence.len() <= STREAM_CARRY_LIMIT {
        return sequence.to_vec();
    }
    let mut carry = sequence[..2].to_vec();
    if sequence.last() == Some(&0x1b) {
        carry.push(0x1b);
    }
    carry
}

/// How many bytes at the end of `line` are the start of a UTF-8 character that has not
/// ended yet.
fn unfinished_char(line: &[u8]) -> usize {
    for back in 1..=line.len().min(4) {
        let byte = line[line.len() - back];
        if byte & 0xc0 == 0x80 {
            continue;
        }
        let width = match byte {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => 1,
        };
        return if width > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests;
