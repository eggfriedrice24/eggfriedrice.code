//! The one public error type of the crate.

use std::path::PathBuf;
use std::time::Duration;

use http::HeaderName;

/// Every way an `efr-http` operation can fail.
///
/// URLs in variants are already redacted (see [`redact::url`](crate::redact::url)),
/// and reqwest errors are stored without their URL, so a logged error never carries a
/// token from a query string. Header values are never stored at all.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpError {
    /// A URL did not parse. The input is not kept because it may hold a secret.
    #[error("the URL is not valid")]
    InvalidUrl {
        /// The error from the URL parser.
        #[source]
        source: url::ParseError,
    },

    /// A URL has a scheme other than `http` or `https`.
    #[error("the URL scheme {scheme:?} is not http or https")]
    UnsupportedScheme {
        /// The scheme.
        scheme: String,
    },

    /// A header value holds bytes that HTTP does not allow, such as a newline. The value
    /// is not kept because it may be a token.
    #[error("the value of the {name} header is not a valid header value")]
    InvalidHeaderValue {
        /// The header.
        name: HeaderName,
    },

    /// The reqwest client could not be built, for example because the TLS roots
    /// could not be loaded.
    #[error("could not build the HTTP client")]
    BuildClient {
        /// The error from reqwest.
        #[source]
        source: reqwest::Error,
    },

    /// No connection to the server could be made. Worth a retry.
    #[error("could not connect to {url}")]
    Connect {
        /// The redacted URL.
        url: String,
        /// The error from reqwest, without its URL.
        #[source]
        source: reqwest::Error,
    },

    /// The server did not answer within a client timeout. Worth a retry when it
    /// happens before the response starts.
    #[error("the request to {url} timed out")]
    Timeout {
        /// The redacted URL.
        url: String,
    },

    /// The request failed after the connection was made, before a response arrived.
    #[error("the request to {url} failed")]
    Send {
        /// The redacted URL.
        url: String,
        /// The error from reqwest, without its URL.
        #[source]
        source: reqwest::Error,
    },

    /// The response body could not be read to the end.
    #[error("could not read the response body from {url}")]
    Body {
        /// The redacted URL.
        url: String,
        /// The error from reqwest, without its URL.
        #[source]
        source: reqwest::Error,
    },

    /// A response body is larger than the client reads into memory.
    #[error("the response body from {url} is larger than {limit} bytes")]
    BodyTooLarge {
        /// The redacted URL.
        url: String,
        /// The largest body the client reads.
        limit: usize,
    },

    /// A request body could not be serialised as JSON.
    #[error("could not serialise the request body as JSON")]
    EncodeJson {
        /// The error from the serialiser.
        #[source]
        source: serde_json::Error,
    },

    /// A response body is not the JSON the caller expected.
    #[error("the response body from {url} is not the expected JSON")]
    DecodeJson {
        /// The redacted URL.
        url: String,
        /// The error from the parser.
        #[source]
        source: serde_json::Error,
    },

    /// A server-sent event, or a line inside one, is larger than the decoder allows.
    #[error("a server-sent event is larger than {limit} bytes")]
    SseEventTooLarge {
        /// The largest event the decoder accepts.
        limit: usize,
    },

    /// The Unix socket could not be opened.
    #[error("could not connect to the socket {}", .socket.display())]
    UnixConnect {
        /// The socket.
        socket: PathBuf,
        /// The error from the operating system.
        #[source]
        source: std::io::Error,
    },

    /// The HTTP exchange over a Unix socket failed.
    #[error("the HTTP exchange over the socket {} failed", .socket.display())]
    UnixExchange {
        /// The socket.
        socket: PathBuf,
        /// The error from hyper.
        #[source]
        source: hyper::Error,
    },

    /// A request for a Unix socket could not be built from its parts.
    #[error("could not build the request for the socket {}", .socket.display())]
    UnixRequest {
        /// The socket.
        socket: PathBuf,
        /// The error from the `http` crate.
        #[source]
        source: http::Error,
    },

    /// The server behind a Unix socket did not answer within the client's timeout,
    /// measured on the injected clock.
    #[error("the socket {} did not answer within {after:?}", .socket.display())]
    UnixTimedOut {
        /// The socket.
        socket: PathBuf,
        /// The timeout.
        after: Duration,
    },
}

impl HttpError {
    /// True for failures that may pass if the same request is sent again: no
    /// connection, or a timeout before the response. The retry policy asks this.
    pub fn is_transient(&self) -> bool {
        matches!(self, HttpError::Connect { .. } | HttpError::Timeout { .. })
    }
}

#[cfg(test)]
mod tests;
