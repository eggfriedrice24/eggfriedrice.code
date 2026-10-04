//! A server-sent events parser over a byte stream.
//!
//! [`SseDecoder`] implements the event stream interpretation of the WHATWG HTML
//! standard (section 9.2.6) over chunks that may split anywhere: inside a line, inside
//! a CRLF pair, inside a UTF-8 sequence or inside the leading byte order mark.
//! [`SseStream`] runs it over a stream of byte chunks, such as a response body.
//!
//! Choices the standard leaves to the reader, or that differ from browsers:
//!
//! - Comment lines (starting with `:`) are dropped; servers send them as keep-alives.
//! - An event that has not been closed by a blank line when the stream ends is
//!   discarded, as the standard requires. A truncated stream therefore loses its last
//!   event instead of delivering half of it.
//! - The decoder never reconnects; it only reports the `retry` value it has seen.
//! - Lines and events larger than a limit (16 MiB by default) are an error rather than
//!   unbounded memory.

use std::collections::VecDeque;
use std::mem;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use futures::Stream;

use crate::HttpError;

const BOM: &[u8] = b"\xEF\xBB\xBF";
const DEFAULT_EVENT_TYPE: &str = "message";

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SseEvent {
    /// The event type: the last `event` field of the event, or `message`.
    pub event: String,
    /// The `data` fields of the event, joined with `\n`.
    pub data: String,
    /// The last event id the stream has set, when it has set one. Like the standard's
    /// last event ID buffer, it carries over to later events until an `id` field
    /// changes it.
    pub id: Option<String>,
}

/// The incremental parser. Feed it chunks with [`SseDecoder::push`].
///
/// After `push` returns an error the decoder is in an unspecified state; the stream it
/// was reading must be abandoned.
#[derive(Debug, Clone)]
pub struct SseDecoder {
    /// The bytes of the line that has no terminator yet.
    line: Vec<u8>,
    /// The previous chunk ended in CR, so a LF at the start of the next one belongs to
    /// the same line terminator.
    skip_lf: bool,
    /// The first line has been seen, so a byte order mark can no longer occur.
    started: bool,
    event_type: String,
    data: String,
    last_id: String,
    retry: Option<Duration>,
    max_event_bytes: usize,
}

impl SseDecoder {
    /// The default limit on the size of one line and of one event's data.
    pub const DEFAULT_MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

    /// A decoder with the default size limit.
    pub fn new() -> Self {
        SseDecoder::with_max_event_bytes(Self::DEFAULT_MAX_EVENT_BYTES)
    }

    /// A decoder that rejects a line or an event's data larger than `limit` bytes.
    pub fn with_max_event_bytes(limit: usize) -> Self {
        SseDecoder {
            line: Vec::new(),
            skip_lf: false,
            started: false,
            event_type: String::new(),
            data: String::new(),
            last_id: String::new(),
            retry: None,
            max_event_bytes: limit,
        }
    }

    /// Parses `chunk` and returns the events it completes, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, HttpError> {
        let mut events = Vec::new();
        let mut rest = chunk;
        if self.skip_lf && !rest.is_empty() {
            self.skip_lf = false;
            if let Some(after) = rest.strip_prefix(b"\n") {
                rest = after;
            }
        }
        while let Some(end) = rest.iter().position(|&b| b == b'\r' || b == b'\n') {
            self.extend_line(&rest[..end])?;
            let terminator = rest[end];
            rest = &rest[end + 1..];
            if terminator == b'\r' {
                match rest.first() {
                    Some(b'\n') => rest = &rest[1..],
                    Some(_) => {}
                    None => self.skip_lf = true,
                }
            }
            let line = mem::take(&mut self.line);
            self.process_line(&line, &mut events)?;
            // Hand the allocation back for the next line.
            self.line = line;
            self.line.clear();
        }
        self.extend_line(rest)?;
        Ok(events)
    }

    /// The last event id the stream has set, for a `Last-Event-ID` header on a
    /// reconnect.
    pub fn last_event_id(&self) -> Option<&str> {
        (!self.last_id.is_empty()).then_some(self.last_id.as_str())
    }

    /// The reconnection delay the server asked for in its last valid `retry` field.
    pub fn retry(&self) -> Option<Duration> {
        self.retry
    }

    fn extend_line(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        if self.line.len() + bytes.len() > self.max_event_bytes {
            return Err(HttpError::SseEventTooLarge { limit: self.max_event_bytes });
        }
        self.line.extend_from_slice(bytes);
        Ok(())
    }

    fn process_line(&mut self, line: &[u8], events: &mut Vec<SseEvent>) -> Result<(), HttpError> {
        let line = if self.started {
            line
        } else {
            self.started = true;
            line.strip_prefix(BOM).unwrap_or(line)
        };
        if line.is_empty() {
            self.dispatch(events);
            return Ok(());
        }
        if line.starts_with(b":") {
            return Ok(());
        }
        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(colon) => {
                let value = &line[colon + 1..];
                (&line[..colon], value.strip_prefix(b" ").unwrap_or(value))
            }
            None => (line, &[][..]),
        };
        match field {
            b"event" => self.event_type = String::from_utf8_lossy(value).into_owned(),
            b"data" => {
                if self.data.len() + value.len() + 1 > self.max_event_bytes {
                    return Err(HttpError::SseEventTooLarge { limit: self.max_event_bytes });
                }
                self.data.push_str(&String::from_utf8_lossy(value));
                self.data.push('\n');
            }
            b"id" if !value.contains(&0) => {
                self.last_id = String::from_utf8_lossy(value).into_owned();
            }
            b"retry" if !value.is_empty() && value.iter().all(u8::is_ascii_digit) => {
                // All digits, so the only failure is overflow, which the standard does
                // not cover; such a value is ignored like any other invalid one.
                if let Ok(millis) = String::from_utf8_lossy(value).parse::<u64>() {
                    self.retry = Some(Duration::from_millis(millis));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        let event_type = mem::take(&mut self.event_type);
        if self.data.is_empty() {
            return;
        }
        let mut data = mem::take(&mut self.data);
        if data.ends_with('\n') {
            data.pop();
        }
        let event = if event_type.is_empty() { DEFAULT_EVENT_TYPE.to_owned() } else { event_type };
        events.push(SseEvent { event, data, id: self.last_event_id().map(str::to_owned) });
    }
}

impl Default for SseDecoder {
    fn default() -> Self {
        SseDecoder::new()
    }
}

/// The events of a stream of byte chunks, such as a response body.
///
/// The stream ends after the first error, whether it came from the inner stream or
/// from the decoder.
#[derive(Debug)]
#[must_use = "a stream does nothing unless it is polled"]
pub struct SseStream<S> {
    inner: S,
    decoder: SseDecoder,
    pending: VecDeque<SseEvent>,
    done: bool,
}

impl<S> SseStream<S> {
    /// Events of `inner` with a default [`SseDecoder`].
    pub fn new(inner: S) -> Self {
        SseStream::with_decoder(inner, SseDecoder::new())
    }

    /// Events of `inner` parsed by `decoder`, for a custom size limit.
    pub fn with_decoder(inner: S, decoder: SseDecoder) -> Self {
        SseStream { inner, decoder, pending: VecDeque::new(), done: false }
    }

    /// The decoder, for [`SseDecoder::last_event_id`] and [`SseDecoder::retry`].
    pub fn decoder(&self) -> &SseDecoder {
        &self.decoder
    }
}

impl<S> Stream for SseStream<S>
where
    S: Stream<Item = Result<Bytes, HttpError>> + Unpin,
{
    type Item = Result<SseEvent, HttpError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            if let Some(event) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if this.done {
                return Poll::Ready(None);
            }
            match ready!(Pin::new(&mut this.inner).poll_next(cx)) {
                Some(Ok(chunk)) => match this.decoder.push(&chunk) {
                    Ok(events) => this.pending.extend(events),
                    Err(error) => {
                        this.done = true;
                        return Poll::Ready(Some(Err(error)));
                    }
                },
                Some(Err(error)) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(error)));
                }
                None => this.done = true,
            }
        }
    }
}

#[cfg(test)]
mod tests;
