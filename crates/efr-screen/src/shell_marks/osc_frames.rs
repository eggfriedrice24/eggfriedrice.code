//! Finds complete OSC sequences in a PTY byte stream.
//!
//! This is the `ESC ]` framing state machine and nothing else: it knows where an OSC
//! starts and ends and hands out the body, and leaves the meaning of the body to its
//! caller. It follows the VT parser's transitions where they matter for framing:
//!
//! - `ESC ]` opens an OSC; `BEL` or `ESC \` closes it.
//! - `ESC P`, `ESC X`, `ESC ^` and `ESC _` open a DCS, SOS, PM or APC string, which is
//!   skipped up to `ESC \`. Its payload (a sixel image, a kitty graphics blob) is never
//!   read as an OSC.
//! - CAN and SUB abort whatever sequence is open.
//! - Other C0 controls are ignored inside a string and do not end an escape.
//!
//! It deliberately differs from the VT parsers in two places:
//!
//! - An `ESC` inside an OSC that is not followed by `\` aborts the OSC without a
//!   frame. libghostty-vt and vte dispatch the OSC there, but no shell emits that
//!   shape, and treating it as noise means a stray `ESC ]` in binary output can
//!   neither produce a false frame nor swallow the next real one.
//! - Inside a skipped string, a doubled `ESC ESC` is payload. That is how tmux wraps
//!   passthrough sequences (`ESC P tmux; ESC ESC ] ... ESC \`), so the marks of a shell
//!   running inside tmux in the hidden shell are not taken for the hidden shell's own.
//!   Any other `ESC` ends the string and starts a new escape sequence, as in the VT
//!   parser, so a stray `ESC P` in binary output cannot hide the next real mark.
//!
//! State survives between calls, so a sequence may be split across chunks anywhere.

use memchr::{memchr, memchr3};

/// The longest OSC body kept. A longer one is dropped up to its terminator, so a
/// program that never terminates an OSC cannot make the scanner buffer without limit.
pub(crate) const MAX_BODY: usize = 4096;

const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ESC: u8 = 0x1b;

/// One complete OSC sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OscFrame<'a> {
    /// The stream offset of the `ESC` that opened the sequence.
    pub(crate) start: u64,
    /// The stream offset just after the terminator.
    pub(crate) end: u64,
    /// The bytes between `ESC ]` and the terminator, without ignored C0 controls.
    pub(crate) body: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// Plain output.
    #[default]
    Ground,
    /// After an `ESC`.
    Escape,
    /// Inside an OSC body.
    Osc,
    /// An `ESC` inside an OSC body.
    OscEscape,
    /// Inside an OSC body that passed [`MAX_BODY`].
    Overflow,
    /// An `ESC` inside an overlong OSC body.
    OverflowEscape,
    /// Inside a DCS, SOS, PM or APC string.
    Skip,
    /// An `ESC` inside a skipped string.
    SkipEscape,
}

/// The framing state machine.
#[derive(Debug, Default)]
pub(crate) struct OscFrames {
    state: State,
    /// The stream offset of the `ESC` that opened the current sequence.
    start: u64,
    body: Vec<u8>,
}

impl OscFrames {
    /// Forgets a partly read sequence, for a gap in the stream.
    pub(crate) fn reset(&mut self) {
        self.state = State::Ground;
        self.body.clear();
    }

    /// Scans `bytes`, whose first byte is at stream offset `base`, and calls
    /// `on_frame` for every OSC that ends inside them.
    pub(crate) fn push(&mut self, bytes: &[u8], base: u64, mut on_frame: impl FnMut(OscFrame<'_>)) {
        let mut index = 0;
        while index < bytes.len() {
            // Long runs of output and of skipped strings hold nothing of interest
            // until the next byte that can change the state.
            let rest = &bytes[index..];
            let found = match self.state {
                State::Ground => memchr(ESC, rest),
                State::Skip => memchr3(ESC, CAN, SUB, rest),
                _ => Some(0),
            };
            let Some(skip) = found else {
                return;
            };
            index += skip;
            let offset = base.saturating_add(index as u64);
            self.step(bytes[index], offset, &mut on_frame);
            index += 1;
        }
    }

    fn step(&mut self, byte: u8, offset: u64, on_frame: &mut impl FnMut(OscFrame<'_>)) {
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.open(offset);
                }
            }
            State::Escape => self.escape(byte, offset),
            State::Osc => match byte {
                BEL => self.finish(offset, on_frame),
                ESC => self.state = State::OscEscape,
                CAN | SUB => self.reset(),
                // The VT parser ignores other C0 controls inside a string.
                0x00..=0x1f => {}
                _ => self.push_body(byte),
            },
            State::OscEscape => {
                if byte == b'\\' {
                    self.finish(offset, on_frame);
                } else {
                    self.reset();
                    self.reopen(byte, offset);
                }
            }
            State::Overflow => match byte {
                BEL | CAN | SUB => self.state = State::Ground,
                ESC => self.state = State::OverflowEscape,
                _ => {}
            },
            State::OverflowEscape => {
                if byte == b'\\' {
                    self.state = State::Ground;
                } else {
                    self.reopen(byte, offset);
                }
            }
            State::Skip => match byte {
                ESC => self.state = State::SkipEscape,
                CAN | SUB => self.state = State::Ground,
                _ => {}
            },
            State::SkipEscape => match byte {
                b'\\' => self.state = State::Ground,
                ESC => self.state = State::Skip,
                _ => self.reopen(byte, offset),
            },
        }
    }

    /// The byte after an `ESC`.
    fn escape(&mut self, byte: u8, offset: u64) {
        match byte {
            b']' => {
                self.body.clear();
                self.state = State::Osc;
            }
            b'P' | b'X' | b'^' | b'_' => self.state = State::Skip,
            ESC => self.open(offset),
            CAN | SUB => self.state = State::Ground,
            // The VT parser executes other C0 controls without leaving the escape.
            0x00..=0x1f => {}
            _ => self.state = State::Ground,
        }
    }

    fn open(&mut self, offset: u64) {
        self.start = offset;
        self.state = State::Escape;
    }

    /// An `ESC` that ended a string without forming `ESC \` begins a new escape
    /// sequence; `byte` is the one after it.
    fn reopen(&mut self, byte: u8, offset: u64) {
        self.open(offset.saturating_sub(1));
        self.escape(byte, offset);
    }

    fn push_body(&mut self, byte: u8) {
        if self.body.len() == MAX_BODY {
            self.body.clear();
            self.state = State::Overflow;
        } else {
            self.body.push(byte);
        }
    }

    /// `offset` is the offset of the last byte of the terminator.
    fn finish(&mut self, offset: u64, on_frame: &mut impl FnMut(OscFrame<'_>)) {
        on_frame(OscFrame { start: self.start, end: offset.saturating_add(1), body: &self.body });
        self.reset();
    }
}

#[cfg(test)]
mod tests;
