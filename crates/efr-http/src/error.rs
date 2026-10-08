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

    /// No connection to the server could be made: DNS, the TCP connect, the TLS
    /// handshake or the connect timeout failed. The request never reached the server,
    /// so a retry is safe for any method.
    #[error("could not connect to {url}")]
    Connect {
        /// The redacted URL.
        url: String,
        /// The error from reqwest, without its URL.
        #[source]
        source: reqwest::Error,
    },

    /// The exchange did not finish within the request's deadline or the read timeout.
    /// The request may have reached the server, so only an idempotent request is sent
    /// again.
    #[error("the request to {url} timed out")]
    Timeout {
        /// The redacted URL.
        url: String,
    },

    /// The request failed after the connection was made, before a response arrived. The
    /// server may have received it, so only an idempotent request is sent again.
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

    /// A WebSocket handshake got an answer other than `101 Switching Protocols`. The
    /// server did not open a socket and did not act on a message.
    #[error("the server at {url} refused the websocket upgrade with {status}")]
    UpgradeRefused {
        /// The redacted URL.
        url: String,
        /// The status of the answer.
        status: http::StatusCode,
    },

    /// A WebSocket handshake got a `101` answer that does not follow RFC 6455.
    #[error("the websocket handshake with {url} failed: {problem}")]
    Handshake {
        /// The redacted URL.
        url: String,
        /// What is wrong with the answer.
        problem: &'static str,
    },

    /// The connection of an accepted WebSocket handshake could not be taken over.
    #[error("could not take over the websocket connection to {url}")]
    Upgrade {
        /// The redacted URL.
        url: String,
        /// The error from reqwest, without its URL.
        #[source]
        source: reqwest::Error,
    },

    /// A WebSocket frame could not be read or written.
    #[error("the websocket connection to {url} failed")]
    WebSocket {
        /// The redacted URL.
        url: String,
        /// The error from the frame codec.
        #[source]
        source: fastwebsockets::WebSocketError,
    },

    /// A message was sent on a WebSocket that is closed.
    #[error("the websocket connection to {url} is closed")]
    WebSocketClosed {
        /// The redacted URL.
        url: String,
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
    /// True when the request certainly never reached the server, because no connection
    /// could be made. Sending it again is safe whatever its method.
    pub fn is_transient(&self) -> bool {
        matches!(self, HttpError::Connect { .. })
    }

    /// True when sending the request again may pass and cannot do harm: always for a
    /// [transient](HttpError::is_transient) failure, and for a timeout or a failure
    /// after the connection was made only when the request is `idempotent`, because the
    /// server may have received it and acted on it.
    pub fn is_retryable(&self, idempotent: bool) -> bool {
        self.is_transient()
            || (idempotent && matches!(self, HttpError::Timeout { .. } | HttpError::Send { .. }))
    }
}

#[cfg(test)]
mod tests;
