//! A response whose body has not been read yet.

use std::fmt;
use std::pin::Pin;

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _};
use http::{HeaderMap, StatusCode};
use serde::de::DeserializeOwned;

use crate::{HttpError, SseStream, redact};

/// The body of a response as a stream of chunks.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, HttpError>> + Send>>;

/// A response: status and headers have arrived, the body is still on the wire.
///
/// Read the body once, in one of three ways: whole ([`bytes`](Self::bytes),
/// [`text`](Self::text), [`json`](Self::json), each capped at the client's body
/// limit), as chunks ([`into_stream`](Self::into_stream)), or as server-sent events
/// ([`into_sse`](Self::into_sse)). Any status comes back as a response; deciding what
/// a 4xx means is the caller's job.
pub struct HttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    url: String,
    body: ByteStream,
    max_body_bytes: usize,
}

impl HttpResponse {
    /// `url` must already be redacted.
    pub(crate) fn new(
        status: StatusCode,
        headers: HeaderMap,
        url: String,
        body: ByteStream,
        max_body_bytes: usize,
    ) -> Self {
        HttpResponse { status, headers, url, body, max_body_bytes }
    }

    /// The status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The request URL, redacted, for log lines and error messages.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The whole body. Fails with [`HttpError::BodyTooLarge`] past the client's limit.
    pub async fn bytes(self) -> Result<Bytes, HttpError> {
        let HttpResponse { url, mut body, max_body_bytes, .. } = self;
        let mut buffer = BytesMut::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            if buffer.len() + chunk.len() > max_body_bytes {
                return Err(HttpError::BodyTooLarge { url, limit: max_body_bytes });
            }
            buffer.extend_from_slice(&chunk);
        }
        Ok(buffer.freeze())
    }

    /// The whole body as text. Bytes that are not UTF-8 become U+FFFD, since the text
    /// is meant for error messages and logs, not for parsing.
    pub async fn text(self) -> Result<String, HttpError> {
        let bytes = self.bytes().await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The whole body parsed as JSON.
    pub async fn json<T: DeserializeOwned>(self) -> Result<T, HttpError> {
        let url = self.url.clone();
        let bytes = self.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|source| HttpError::DecodeJson { url, source })
    }

    /// The body as a stream of chunks, without a size limit.
    pub fn into_stream(self) -> ByteStream {
        self.body
    }

    /// The body as server-sent events, with the default [`SseDecoder`](crate::SseDecoder).
    pub fn into_sse(self) -> SseStream<ByteStream> {
        SseStream::new(self.body)
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("url", &self.url)
            .field("headers", &redact::headers(&self.headers))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
