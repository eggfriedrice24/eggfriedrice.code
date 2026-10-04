//! The backend-independent tap for OSC 133 and OSC 7.
//!
//! Neither screen backend reports semantic prompt marks with an exit code and a
//! stream offset: libghostty-vt keeps only per-row prompt state, and vt100 handles
//! only OSC 0, 1, 2 and 52. So the actor runs a [`ShellMarkScanner`] over every chunk
//! before the backend sees it, and every backend gets the same marks with the same
//! recording offsets. Three small parts: `osc_frames` finds complete OSC sequences,
//! `semantic_prompt` reads OSC 133 and `cwd_report` reads OSC 7.

mod cwd_report;
mod osc_frames;
mod semantic_prompt;

use std::path::PathBuf;

use efr_protocol::Seq;

use self::osc_frames::OscFrames;
pub use self::semantic_prompt::{ClickMode, PromptKind, SemanticPromptEvent};

/// One shell mark found in the PTY stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellMark {
    /// The recording offset of the `ESC` that opened the sequence.
    pub start: Seq,
    /// The recording offset just after the sequence's terminator.
    pub end: Seq,
    /// What the mark says.
    pub kind: ShellMarkKind,
}

/// What a [`ShellMark`] says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShellMarkKind {
    /// An OSC 133 semantic prompt mark.
    SemanticPrompt(SemanticPromptEvent),
    /// An OSC 7 report of the shell's working directory.
    CwdChanged {
        /// The host named in the URL; `None` when the URL leaves it empty. The scanner
        /// does not check that it is the local host.
        host: Option<String>,
        /// The absolute path, decoded from the URL.
        path: PathBuf,
    },
}

/// Finds OSC 133 and OSC 7 marks in a PTY byte stream.
///
/// Feed it the stream in order, chunk by chunk, with the recording offset of each
/// chunk's first byte. A sequence may be split across chunks anywhere. When a chunk
/// does not start where the previous one ended (a gap in the recording), a sequence
/// that was partly read is dropped rather than glued to unrelated bytes.
#[derive(Debug, Default)]
pub struct ShellMarkScanner {
    frames: OscFrames,
    /// Where the next chunk is expected to start; `None` before the first chunk.
    next: Option<Seq>,
}

impl ShellMarkScanner {
    /// A scanner that has seen nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scans `bytes`, whose first byte is at recording offset `base`, and returns the
    /// marks that end inside them, in stream order. A mark that started in an
    /// earlier chunk keeps that chunk's start offset.
    pub fn scan(&mut self, bytes: &[u8], base: Seq) -> Vec<ShellMark> {
        if self.next.is_some_and(|next| next != base) {
            self.frames.reset();
        }
        self.next = Some(Seq::new(base.get().saturating_add(bytes.len() as u64)));
        let mut marks = Vec::new();
        self.frames.push(bytes, base.get(), |frame| {
            if let Some(kind) = classify(frame.body) {
                marks.push(ShellMark {
                    start: Seq::new(frame.start),
                    end: Seq::new(frame.end),
                    kind,
                });
            }
        });
        marks
    }
}

/// The mark an OSC body holds, if it is one this scanner reads.
fn classify(body: &[u8]) -> Option<ShellMarkKind> {
    let separator = body.iter().position(|&byte| byte == b';')?;
    let (number, rest) = body.split_at(separator);
    let data = &rest[1..];
    match number {
        b"133" => semantic_prompt::parse(data).map(ShellMarkKind::SemanticPrompt),
        b"7" => cwd_report::parse(data)
            .map(|report| ShellMarkKind::CwdChanged { host: report.host, path: report.path }),
        _ => None,
    }
}

/// Decodes `%XX` escapes. A `%` that is not followed by two hex digits makes the
/// input undecodable, as in ghostty's decoder.
fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut bytes = input.iter();
    while let Some(&byte) = bytes.next() {
        if byte == b'%' {
            let high = hex_digit(*bytes.next()?)?;
            let low = hex_digit(*bytes.next()?)?;
            decoded.push(high << 4 | low);
        } else {
            decoded.push(byte);
        }
    }
    Some(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).and_then(|digit| u8::try_from(digit).ok())
}

#[cfg(test)]
mod tests;
