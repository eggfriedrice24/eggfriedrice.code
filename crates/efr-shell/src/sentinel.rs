//! The random-token sentinel: delimiting a command in a shell that has no efr
//! integration (a `sudo -i` or `bash` inside the hidden zsh, or a shell that is not a
//! zsh at all).
//!
//! The typed line is
//!
//! ```text
//! printf '__efr_%s_b\n' TOKEN; eval 'COMMAND'; printf '\n__efr_%s_e:%s:%s\n' TOKEN "$?" "$PWD"
//! ```
//!
//! so the shell prints `__efr_TOKEN_b` before the output and `__efr_TOKEN_e:STATUS:PWD`
//! after it. The terminal echoes the typed line too, but there the token is never
//! next to `__efr_`, so the echo cannot match. `eval` keeps a command with `;`, `&` or
//! a pipe in one place, and its status is the command's. The token comes from the
//! injected generator, so output that merely looks like a marker cannot end a run.

use std::ffi::OsString;
use std::ops::Range;
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

use bytes::Bytes;
use efr_protocol::Seq;
use efr_stdx::rng::Rng;

use crate::ShellError;
use crate::capture::{Capture, Kept};
use crate::run::{Completion, RunOutput};

/// A new token: 16 hex digits from `rng`.
pub(crate) fn token(rng: &dyn Rng) -> String {
    format!("{:016x}", rng.next_u64())
}

/// The bytes that type `command` with sentinels around it, then Enter.
pub(crate) fn sentinel_line(command: &str, token: &str) -> Result<Bytes, ShellError> {
    if command.contains('\0') {
        return Err(ShellError::InvalidCommand { reason: "it contains a NUL byte" });
    }
    if command.trim().is_empty() {
        return Err(ShellError::InvalidCommand { reason: "it is empty" });
    }
    let line = format!(
        "printf '__efr_%s_b\\n' {token}; eval {}; printf '\\n__efr_%s_e:%s:%s\\n' {token} \"$?\" \"$PWD\"\r",
        quote(command)
    );
    Ok(Bytes::from(line))
}

/// `command` as one shell word. Plain single quotes when it has no control
/// characters; otherwise `$'...'` with escapes, so that no newline or tab is typed
/// into a line editor, where it would end the line or start a completion.
fn quote(command: &str) -> String {
    if !command.chars().any(char::is_control) {
        return format!("'{}'", command.replace('\'', r"'\''"));
    }
    let mut quoted = String::with_capacity(command.len() + 3);
    quoted.push_str("$'");
    for c in command.chars() {
        match c {
            '\\' => quoted.push_str(r"\\"),
            '\'' => quoted.push_str(r"\'"),
            '\n' => quoted.push_str(r"\n"),
            '\t' => quoted.push_str(r"\t"),
            '\r' => quoted.push_str(r"\r"),
            c if c.is_control() => {
                let mut buf = [0; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    quoted.push_str(&format!(r"\x{byte:02x}"));
                }
            }
            c => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}

/// Where a sentinel run is in the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Looking for `__efr_TOKEN_b`.
    SeekBegin,
    /// The begin marker was seen; waiting for the end of its line.
    SkipBeginLine,
    /// Keeping output, looking for `__efr_TOKEN_e:`.
    Output,
    /// The end marker was seen; reading `STATUS:PWD` up to the end of its line.
    EndLine,
}

/// A run delimited by sentinels. It reads the bytes after the line was typed and
/// ends at the end marker's line.
#[derive(Debug)]
pub(crate) struct SentinelRun {
    begin: Vec<u8>,
    end: Vec<u8>,
    stage: Stage,
    /// Bytes that may still be part of a marker, starting at stream offset `held_at`.
    held: Vec<u8>,
    held_at: u64,
    capture: Capture,
    output_start: Option<Seq>,
    /// The offset after the last byte captured.
    captured_end: Seq,
    /// The offset where the output ended, once the end marker was found.
    output_end: Option<Seq>,
    /// Type [`FORGET_CREDENTIALS`](crate::run::FORGET_CREDENTIALS) at the outer zsh's
    /// next prompt once the run ends.
    forget_credentials: bool,
}

impl SentinelRun {
    pub(crate) fn new(token: &str, output_limit: usize) -> Self {
        SentinelRun {
            begin: format!("__efr_{token}_b").into_bytes(),
            end: format!("__efr_{token}_e:").into_bytes(),
            stage: Stage::SeekBegin,
            held: Vec::new(),
            held_at: 0,
            capture: Capture::new(output_limit),
            output_start: None,
            captured_end: Seq::ZERO,
            output_end: None,
            forget_credentials: false,
        }
    }

    /// The same run, making the shell forget the cached credentials when it ends when
    /// `forget` is set.
    pub(crate) fn forgetting(mut self, forget: bool) -> Self {
        self.forget_credentials = forget;
        self
    }

    /// True when the shell must forget the cached credentials once the run ends.
    pub(crate) fn forgets_credentials(&self) -> bool {
        self.forget_credentials
    }

    pub(crate) fn capture(&self) -> &Capture {
        &self.capture
    }

    /// True between the begin marker's line and the end marker: the command runs.
    pub(crate) fn running(&self) -> bool {
        self.stage == Stage::Output
    }

    /// The output so far, for a run that is left running.
    pub(crate) fn partial(&self) -> (Kept, Option<Range<Seq>>) {
        (self.capture.finish(), self.output_start.map(|start| start..self.captured_end))
    }

    /// Takes stream bytes at offset `at`. Returns the output when the run ended, with
    /// `captured` true when some of the bytes were output.
    pub(crate) fn on_bytes(&mut self, at: Seq, bytes: &[u8]) -> (Option<RunOutput>, bool) {
        let expected = self.held_at.saturating_add(self.held.len() as u64);
        if !self.held.is_empty() && at.get() != expected {
            // Something (a mark) was cut out of the stream here; a marker cannot span
            // it, so what is held is settled now.
            self.settle_held();
        }
        if self.held.is_empty() {
            self.held_at = at.get();
        }
        self.held.extend_from_slice(bytes);
        let before = self.capture.total();
        let ended = self.advance();
        (ended, self.capture.total() != before)
    }

    fn advance(&mut self) -> Option<RunOutput> {
        loop {
            match self.stage {
                Stage::SeekBegin => {
                    if let Some(found) = find(&self.held, &self.begin) {
                        self.consume(found + self.begin.len());
                        self.stage = Stage::SkipBeginLine;
                    } else {
                        let keep = self.begin.len().saturating_sub(1);
                        self.consume(self.held.len().saturating_sub(keep));
                        return None;
                    }
                }
                Stage::SkipBeginLine => {
                    let Some(newline) = self.held.iter().position(|&b| b == b'\n') else {
                        self.consume(self.held.len());
                        return None;
                    };
                    self.consume(newline + 1);
                    self.output_start = Some(Seq::new(self.held_at));
                    self.captured_end = Seq::new(self.held_at);
                    self.stage = Stage::Output;
                }
                Stage::Output => {
                    if let Some(found) = find(&self.held, &self.end) {
                        // The `\n` printed before the marker is not output.
                        let output = strip_line_end(&self.held[..found]).len();
                        self.commit(output);
                        self.output_end = Some(self.captured_end);
                        self.consume(found - output + self.end.len());
                        self.stage = Stage::EndLine;
                    } else {
                        // Keep enough for a marker that is not complete yet, and the
                        // line end before it.
                        let keep = self.end.len() + 1;
                        self.commit(self.held.len().saturating_sub(keep));
                        return None;
                    }
                }
                Stage::EndLine => {
                    let newline = self.held.iter().position(|&b| b == b'\n')?;
                    let line = strip_line_end(&self.held[..=newline]).to_vec();
                    self.consume(newline + 1);
                    return Some(self.finish(&line));
                }
            }
        }
    }

    /// Settles held bytes that cannot be part of a marker because the stream jumps.
    fn settle_held(&mut self) {
        match self.stage {
            Stage::Output => self.commit(self.held.len()),
            Stage::SeekBegin | Stage::SkipBeginLine => self.consume(self.held.len()),
            Stage::EndLine => {}
        }
    }

    /// Moves the first `n` held bytes into the capture.
    fn commit(&mut self, n: usize) {
        self.capture.push(&self.held[..n]);
        self.consume(n);
        self.captured_end = Seq::new(self.held_at);
    }

    /// Drops the first `n` held bytes.
    fn consume(&mut self, n: usize) {
        self.held.drain(..n);
        self.held_at = self.held_at.saturating_add(n as u64);
    }

    fn finish(&self, line: &[u8]) -> RunOutput {
        let (status, cwd) = match line.iter().position(|&b| b == b':') {
            Some(colon) => (&line[..colon], &line[colon + 1..]),
            None => (line, &[][..]),
        };
        let exit_code = std::str::from_utf8(status).ok().and_then(|text| text.parse().ok());
        let cwd = (!cwd.is_empty()).then(|| PathBuf::from(OsString::from_vec(cwd.to_vec())));
        let range = match (self.output_start, self.output_end) {
            (Some(start), Some(end)) => Some(start..end),
            _ => None,
        };
        RunOutput {
            completion: Completion::Finished,
            exit_code,
            kept: self.capture.finish(),
            range,
            cwd,
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// `bytes` without one trailing `\r\n`, `\n` or `\r`.
fn strip_line_end(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

#[cfg(test)]
mod tests;
