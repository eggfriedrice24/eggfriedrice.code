//! A command's output as it arrives: kept within a byte limit (head and tail), and
//! the byte cleaner that turns terminal bytes into plain text. Output that moves the
//! cursor is read on a capture screen instead (`replay.rs`).

use std::collections::VecDeque;

use bytes::Bytes;

/// The raw output of one run, kept within `limit` bytes: the first half of the limit
/// from the start of the output and the second half from its end. The full output
/// stays in the recording.
#[derive(Debug)]
pub(crate) struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_cap: usize,
    tail_cap: usize,
    /// Every byte pushed, kept or not.
    total: u64,
}

/// The bytes a capture kept: the start of the output and, when bytes were dropped
/// from the middle, its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Kept {
    /// The output from its start: all of it when nothing was dropped.
    pub(crate) head: Bytes,
    /// The output's end after the dropped middle; empty when nothing was dropped.
    pub(crate) tail: Bytes,
    /// How many bytes were dropped between `head` and `tail`.
    pub(crate) dropped: u64,
    /// The size of the whole output in bytes.
    pub(crate) bytes: u64,
}

/// A finished capture as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Captured {
    /// The output as plain text. When bytes were dropped, a marker line stands where
    /// they were.
    pub(crate) text: String,
    /// True when bytes were dropped from the middle.
    pub(crate) truncated: bool,
    /// The size of the whole output in bytes.
    pub(crate) bytes: u64,
}

impl Capture {
    /// A capture that keeps at most `limit` bytes (at least two).
    pub(crate) fn new(limit: usize) -> Self {
        let limit = limit.max(2);
        let head_cap = limit / 2;
        Capture {
            head: Vec::new(),
            tail: VecDeque::new(),
            head_cap,
            tail_cap: limit - head_cap,
            total: 0,
        }
    }

    pub(crate) fn push(&mut self, mut bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        if self.head.len() < self.head_cap {
            let take = bytes.len().min(self.head_cap - self.head.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        if bytes.len() >= self.tail_cap {
            self.tail.clear();
            self.tail.extend(&bytes[bytes.len() - self.tail_cap..]);
            return;
        }
        let overflow = (self.tail.len() + bytes.len()).saturating_sub(self.tail_cap);
        self.tail.drain(..overflow);
        self.tail.extend(bytes);
    }

    /// Takes back the last `n` bytes pushed, such as the start of an OSC 133 `D`
    /// sequence that arrived before the scanner could tell it was one.
    pub(crate) fn trim_end(&mut self, n: usize) {
        let contiguous = self.total == self.kept();
        let from_tail = n.min(self.tail.len());
        self.tail.truncate(self.tail.len() - from_tail);
        // Bytes in the head are the latest ones only when nothing was dropped between;
        // otherwise the rest of `n` comes out of the dropped middle.
        if contiguous {
            let from_head = (n - from_tail).min(self.head.len());
            self.head.truncate(self.head.len() - from_head);
        }
        self.total = self.total.saturating_sub(n as u64);
    }

    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    /// The last `n` bytes pushed (fewer when fewer are kept), for a progress preview.
    pub(crate) fn tail(&self, n: usize) -> Bytes {
        if self.tail.is_empty() {
            let start = self.head.len().saturating_sub(n);
            return Bytes::copy_from_slice(&self.head[start..]);
        }
        let mut last: Vec<u8> = Vec::with_capacity(n);
        let from_tail = n.min(self.tail.len());
        if from_tail < n && self.total == self.kept() {
            let from_head = (n - from_tail).min(self.head.len());
            last.extend_from_slice(&self.head[self.head.len() - from_head..]);
        }
        last.extend(self.tail.iter().skip(self.tail.len() - from_tail));
        Bytes::from(last)
    }

    /// The bytes kept so far. The capture goes on, so a run left running can be read
    /// again later.
    pub(crate) fn finish(&self) -> Kept {
        let dropped = self.total.saturating_sub(self.kept());
        let mut head = self.head.clone();
        if dropped == 0 {
            head.extend(&self.tail);
            return Kept {
                head: Bytes::from(head),
                tail: Bytes::new(),
                dropped,
                bytes: self.total,
            };
        }
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        Kept { head: Bytes::from(head), tail: Bytes::from(tail), dropped, bytes: self.total }
    }

    fn kept(&self) -> u64 {
        (self.head.len() + self.tail.len()) as u64
    }
}

impl Kept {
    /// True when bytes were dropped from the middle.
    pub(crate) fn truncated(&self) -> bool {
        self.dropped > 0
    }

    /// The text through the byte cleaner alone.
    pub(crate) fn clean(&self) -> Captured {
        let head = clean(&self.head);
        let text =
            if self.truncated() { join(&head, self.dropped, &clean(&self.tail)) } else { head };
        Captured { text, truncated: self.truncated(), bytes: self.bytes }
    }
}

/// The text of a truncated output: its head, a marker line for the `dropped` bytes, and
/// its tail.
pub(crate) fn join(head: &str, dropped: u64, tail: &str) -> String {
    format!("{head}\n[... {dropped} bytes omitted ...]\n{tail}")
}

/// Terminal output as plain text: escape sequences (CSI, OSC, DCS and the like)
/// dropped, `\r\n` as `\n`, a bare `\r` letting the text after it replace the line
/// (so a progress bar leaves its last state), a backspace removing the character
/// before it, and other control characters except tab dropped. Bytes that are not
/// UTF-8 become U+FFFD.
pub(crate) fn clean(bytes: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut line_start = 0;
    let mut carriage_return = false;
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        match byte {
            0x1b => {
                at = skip_escape(bytes, at);
                continue;
            }
            b'\n' => {
                out.push(b'\n');
                line_start = out.len();
                carriage_return = false;
            }
            b'\r' => carriage_return = true,
            0x08 => {
                // A whole UTF-8 character goes: its continuation bytes, then its lead.
                while out.len() > line_start {
                    match out.pop() {
                        Some(byte) if byte & 0xc0 == 0x80 => {}
                        _ => break,
                    }
                }
            }
            b'\t' => push_text(&mut out, line_start, &mut carriage_return, byte),
            0x00..=0x1f | 0x7f => {}
            _ => push_text(&mut out, line_start, &mut carriage_return, byte),
        }
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn push_text(out: &mut Vec<u8>, line_start: usize, carriage_return: &mut bool, byte: u8) {
    if *carriage_return {
        out.truncate(line_start);
        *carriage_return = false;
    }
    out.push(byte);
}

/// The index after the escape sequence that starts at `start` (an `ESC`), or the end
/// of `bytes` when the sequence is not complete.
pub(crate) fn skip_escape(bytes: &[u8], start: usize) -> usize {
    let Some(&kind) = bytes.get(start + 1) else {
        return bytes.len();
    };
    let body = start + 2;
    match kind {
        // CSI: parameters and intermediates, then one final byte.
        b'[' => bytes[body.min(bytes.len())..]
            .iter()
            .position(|b| (0x40..=0x7e).contains(b))
            .map_or(bytes.len(), |end| body + end + 1),
        // OSC: up to BEL or ST.
        b']' => string_end(bytes, body, true),
        // DCS, SOS, PM, APC: up to ST.
        b'P' | b'X' | b'^' | b'_' => string_end(bytes, body, false),
        // Character set designations take one more byte.
        b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/' | b'#' | b'%' => {
            (start + 3).min(bytes.len())
        }
        _ => start + 2,
    }
}

/// The index after the terminator of a control string that starts at `body`: ST
/// (`ESC \`), or BEL too when `bel` is true.
fn string_end(bytes: &[u8], body: usize, bel: bool) -> usize {
    let mut at = body;
    while at < bytes.len() {
        match bytes[at] {
            0x07 if bel => return at + 1,
            0x1b if bytes.get(at + 1) == Some(&b'\\') => return at + 2,
            _ => at += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests;
