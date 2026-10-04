//! The transcript hook.
//!
//! With `EFR_RECORD_TRANSCRIPT` set, the daemon gives its [`HttpClient`] a
//! [`Recorder`] that writes provider traffic to an NDJSON transcript, which is how
//! replay fixtures are captured from a live session. This module only defines what
//! the client reports; the file format belongs to the daemon and the test support
//! crate.
//!
//! Only requests marked with [`HttpRequest::recorded`] are reported, and headers and
//! URLs arrive already redacted.
//!
//! [`HttpClient`]: crate::HttpClient
//! [`HttpRequest::recorded`]: crate::HttpRequest::recorded

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use futures::Stream;
use http::{HeaderMap, Method, StatusCode};

use crate::{ByteStream, HttpError};

/// Receives the events of recorded exchanges.
///
/// `record` runs on the task that drives the request, between polls of the response
/// body, so it must be quick: append to a buffer or a channel, not to a slow disk.
pub trait Recorder: Send + Sync + fmt::Debug {
    /// Called once per event, in order within an exchange.
    fn record(&self, record: &Record<'_>);
}

/// Numbers the exchanges of one client, so that the records of concurrent requests
/// can be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExchangeId(pub(crate) u64);

impl ExchangeId {
    /// The number, starting at 1 for the first exchange of a client.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ExchangeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One event of a recorded exchange.
#[derive(Debug)]
#[non_exhaustive]
pub enum Record<'a> {
    /// The request is about to be sent.
    Request {
        /// The exchange.
        exchange: ExchangeId,
        /// The method.
        method: &'a Method,
        /// The redacted URL.
        url: &'a str,
        /// The redacted headers.
        headers: &'a HeaderMap,
        /// The body as sent.
        body: &'a [u8],
    },
    /// The request failed before a response arrived.
    Failed {
        /// The exchange.
        exchange: ExchangeId,
        /// The failure.
        error: &'a HttpError,
    },
    /// Status and headers arrived.
    Response {
        /// The exchange.
        exchange: ExchangeId,
        /// The status.
        status: StatusCode,
        /// The redacted headers.
        headers: &'a HeaderMap,
    },
    /// A chunk of the response body arrived.
    BodyChunk {
        /// The exchange.
        exchange: ExchangeId,
        /// The chunk.
        bytes: &'a [u8],
    },
    /// The response body ended.
    BodyEnd {
        /// The exchange.
        exchange: ExchangeId,
        /// How it ended.
        end: BodyEnd,
    },
}

/// How a recorded response body ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BodyEnd {
    /// The server finished the body.
    Complete,
    /// Reading the body failed.
    Failed,
    /// The caller dropped the body before its end, as an interrupted turn does.
    Dropped,
}

/// `body` with every chunk and its end reported to `recorder`.
pub(crate) fn record_body(
    body: ByteStream,
    recorder: Arc<dyn Recorder>,
    exchange: ExchangeId,
) -> ByteStream {
    Box::pin(RecordedBody { body, recorder, exchange, ended: false })
}

struct RecordedBody {
    body: ByteStream,
    recorder: Arc<dyn Recorder>,
    exchange: ExchangeId,
    ended: bool,
}

impl RecordedBody {
    fn end(&mut self, end: BodyEnd) {
        if !self.ended {
            self.ended = true;
            self.recorder.record(&Record::BodyEnd { exchange: self.exchange, end });
        }
    }
}

impl Stream for RecordedBody {
    type Item = Result<Bytes, HttpError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let item = ready!(this.body.as_mut().poll_next(cx));
        match &item {
            Some(Ok(bytes)) => {
                this.recorder.record(&Record::BodyChunk { exchange: this.exchange, bytes });
            }
            Some(Err(_)) => this.end(BodyEnd::Failed),
            None => this.end(BodyEnd::Complete),
        }
        Poll::Ready(item)
    }
}

impl Drop for RecordedBody {
    fn drop(&mut self) {
        self.end(BodyEnd::Dropped);
    }
}

#[cfg(test)]
mod tests;
