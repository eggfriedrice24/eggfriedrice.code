//! A reader of newline-delimited JSON over a byte stream, with a cap on each line.
//!
//! Some local daemons stream one JSON value per line on a body that never ends, such as
//! tailscaled's `watch-ipn-bus`. [`HttpResponse::bytes`](crate::HttpResponse::bytes)
//! cannot read such a body, and it caps the whole body, not one value. [`JsonLines`]
//! reads one line at a time and holds at most one line (plus the rest of the chunk it
//! came in) in memory.

use bytes::{Bytes, BytesMut};
use futures::StreamExt as _;
use serde::de::DeserializeOwned;

use crate::{ByteStream, HttpError};

/// Reads a body line by line. A line ends at `\n`; a `\r` before it is dropped. A last
/// line without `\n` is still a line.
///
/// [`next_line`](Self::next_line) and [`next_json`](Self::next_json) are cancel safe:
/// a call that is dropped while it waits loses no bytes, so the reader can sit in a
/// `select!`. After either returns an error, abandon the reader.
pub struct JsonLines {
    body: ByteStream,
    url: String,
    buffer: BytesMut,
    /// The bytes at the start of `buffer` that hold no `\n`, so a search starts after
    /// them.
    scanned: usize,
    max_line_bytes: usize,
    ended: bool,
}

impl JsonLines {
    /// The default for the line cap.
    pub const DEFAULT_MAX_LINE_BYTES: usize = 1024 * 1024;

    /// A reader of `body`, a response from the redacted `url`, that refuses a line
    /// longer than `max_line_bytes` (without its line end).
    pub(crate) fn new(body: ByteStream, url: String, max_line_bytes: usize) -> Self {
        JsonLines { body, url, buffer: BytesMut::new(), scanned: 0, max_line_bytes, ended: false }
    }

    /// The largest line this reader accepts, in bytes.
    pub fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    /// The next line without its line end, or `None` at the end of the body.
    ///
    /// Fails with [`HttpError::LineTooLarge`] as soon as a line passes the cap, before
    /// the rest of it arrives.
    pub async fn next_line(&mut self) -> Result<Option<Bytes>, HttpError> {
        loop {
            if let Some(line) = self.take_line()? {
                return Ok(Some(line));
            }
            if self.ended {
                if self.buffer.is_empty() {
                    return Ok(None);
                }
                let line = self.buffer.split().freeze();
                self.scanned = 0;
                return Ok(Some(strip_cr(line)));
            }
            // NOTE: the only await. Every change to `self` happens after it returns,
            // so a dropped call loses nothing.
            match self.body.next().await {
                Some(Ok(chunk)) => self.buffer.extend_from_slice(&chunk),
                Some(Err(error)) => return Err(error),
                None => self.ended = true,
            }
        }
    }

    /// The next line that is not blank, parsed as JSON, or `None` at the end of the
    /// body.
    pub async fn next_json<T: DeserializeOwned>(&mut self) -> Result<Option<T>, HttpError> {
        loop {
            let Some(line) = self.next_line().await? else { return Ok(None) };
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return serde_json::from_slice(&line)
                .map(Some)
                .map_err(|source| HttpError::DecodeJson { url: self.url.clone(), source });
        }
    }

    /// A whole line from the buffer, if one is there. Checks the cap on a whole line
    /// and on the part of a line that has no end yet.
    fn take_line(&mut self) -> Result<Option<Bytes>, HttpError> {
        let found = self.buffer[self.scanned..].iter().position(|byte| *byte == b'\n');
        match found {
            Some(offset) => {
                let end = self.scanned + offset;
                let mut line = self.buffer.split_to(end + 1);
                line.truncate(end);
                self.scanned = 0;
                let line = strip_cr(line.freeze());
                if line.len() > self.max_line_bytes {
                    return Err(self.too_large());
                }
                Ok(Some(line))
            }
            None => {
                self.scanned = self.buffer.len();
                // A `\r` at the end may still belong to the line end.
                let open = self.buffer.len() - usize::from(self.buffer.last() == Some(&b'\r'));
                if open > self.max_line_bytes {
                    return Err(self.too_large());
                }
                Ok(None)
            }
        }
    }

    fn too_large(&mut self) -> HttpError {
        self.buffer.clear();
        self.scanned = 0;
        HttpError::LineTooLarge { url: self.url.clone(), limit: self.max_line_bytes }
    }
}

/// `line` without one `\r` at its end.
fn strip_cr(mut line: Bytes) -> Bytes {
    if line.last() == Some(&b'\r') {
        line.truncate(line.len() - 1);
    }
    line
}

impl std::fmt::Debug for JsonLines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonLines")
            .field("url", &self.url)
            .field("buffered", &self.buffer.len())
            .field("max_line_bytes", &self.max_line_bytes)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
