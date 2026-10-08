//! HTTP for efr.
//!
//! - [`HttpClient`]: the one outbound client, reqwest over rustls with connect and
//!   read timeouts, sending an [`HttpRequest`] and returning an [`HttpResponse`].
//! - [`RetryPolicy`]: exponential backoff with jitter and `Retry-After`, waiting on
//!   the injected `efr_stdx::time::Clock` so tests never sleep.
//! - [`SseDecoder`] and [`SseStream`]: server-sent events over a byte stream, for the
//!   streaming Responses API.
//! - [`WebSocket`]: a WebSocket client connection that
//!   [`HttpClient::websocket`] opens over the same TLS stack, for the Responses API's
//!   WebSocket transport.
//! - [`UnixClient`]: HTTP/1.1 over a Unix socket through hyper, for tailscaled's
//!   LocalAPI at the phone milestone.
//! - [`redact`]: header and URL redaction for logs, errors and transcripts.
//! - [`Recorder`]: the hook through which the daemon records provider traffic as an
//!   NDJSON transcript.
//!
//! Allowed dependencies: `efr-stdx` only. What does not belong here: any provider's
//! endpoints, headers or event names (`efr-provider-openai`), token handling
//! (`efr-oauth-openai`), and the transcript file format (`efr-daemon`,
//! `efr-test-support`).
//!
//! The `http` types that appear in this API are re-exported, so a caller needs no
//! direct dependency on `http`, `reqwest` or `hyper`.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

mod client;
mod error;
mod recorder;
pub mod redact;
mod request;
mod response;
mod retry;
mod sse;
#[cfg(test)]
mod testing;
mod unix;
mod websocket;

pub use bytes::Bytes;
pub use client::{HttpClient, HttpConfig};
pub use error::HttpError;
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
pub use recorder::{BodyEnd, ExchangeId, Record, Recorder};
pub use request::HttpRequest;
pub use response::{ByteStream, HttpResponse};
pub use retry::{Decision, Outcome, RetryPolicy, Retryable, is_retryable_status, retry_after};
pub use sse::{SseDecoder, SseEvent, SseStream};
pub use unix::UnixClient;
pub use url::Url;
pub use websocket::{WebSocket, WsMessage, websocket_accept};
