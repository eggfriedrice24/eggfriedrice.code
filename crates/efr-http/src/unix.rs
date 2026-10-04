//! HTTP/1.1 over a Unix socket, for local daemons such as tailscaled's LocalAPI.
//!
//! The phone milestone asks tailscaled who a tailnet peer is with
//! `GET http://local-tailscaled.sock/localapi/v0/whois?addr=<ip:port>` over
//! `/var/run/tailscale/tailscaled.sock`. This client sends any [`HttpRequest`] that
//! way: the URL's host becomes the `Host` header, its path and query become the
//! request target, and the bytes go to the socket instead of a TCP connection.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock;
use futures::TryStreamExt as _;
use http::HeaderValue;
use http::header::HOST;
use http_body_util::{BodyDataStream, Full};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

use crate::{ByteStream, HttpError, HttpRequest, HttpResponse, redact};

/// Sends requests over one Unix socket, one connection per request.
///
/// A local socket costs almost nothing to connect, and a fresh connection per request
/// means no pool to go stale when the daemon behind it restarts. The timeout covers
/// connecting, sending and the response head, measured on the injected [`Clock`]; the
/// body is read afterwards without one, as LocalAPI bodies are small.
#[derive(Debug, Clone)]
pub struct UnixClient {
    socket: PathBuf,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    max_body_bytes: usize,
}

impl UnixClient {
    /// The default for [`UnixClient::with_timeout`].
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
    /// The largest body [`HttpResponse::bytes`] reads from this client.
    pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

    /// A client for the socket at `socket`.
    pub fn new(socket: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        UnixClient {
            socket: socket.into(),
            clock,
            timeout: Self::DEFAULT_TIMEOUT,
            max_body_bytes: Self::MAX_BODY_BYTES,
        }
    }

    /// The same client with a different timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The socket.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Sends `request` and returns the response head with the body still to read.
    ///
    /// Must run inside a tokio runtime: the connection is driven by a task spawned on
    /// it, which ends when the response body is read or dropped.
    pub async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let url = request.url();
        let authority = match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_owned(),
            (None, _) => String::new(),
        };
        let target = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        let mut headers = request.headers().clone();
        if !headers.contains_key(HOST) {
            let host = HeaderValue::from_str(&authority)
                .map_err(|_| HttpError::InvalidHeaderValue { name: HOST })?;
            headers.insert(HOST, host);
        }
        let mut outgoing = http::Request::builder()
            .method(request.method().clone())
            .uri(target)
            .body(Full::new(request.body_bytes().clone()))
            .map_err(|source| HttpError::UnixRequest { socket: self.socket.clone(), source })?;
        *outgoing.headers_mut() = headers;

        let exchange = async {
            let stream = UnixStream::connect(&self.socket)
                .await
                .map_err(|source| HttpError::UnixConnect { socket: self.socket.clone(), source })?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|source| self.exchange_error(source))?;
            // The connection's own result repeats what `send_request` and the body
            // stream report, so it is dropped here.
            tokio::spawn(async move {
                let _ = connection.await;
            });
            sender.send_request(outgoing).await.map_err(|source| self.exchange_error(source))
        };
        let response = self.clock.timeout(self.timeout, exchange).await.map_err(|_| {
            HttpError::UnixTimedOut { socket: self.socket.clone(), after: self.timeout }
        })??;

        let (parts, body) = response.into_parts();
        let socket = self.socket.clone();
        let body: ByteStream = Box::pin(
            BodyDataStream::new(body)
                .map_err(move |source| HttpError::UnixExchange { socket: socket.clone(), source }),
        );
        Ok(HttpResponse::new(
            parts.status,
            parts.headers,
            redact::url(url),
            body,
            self.max_body_bytes,
        ))
    }

    fn exchange_error(&self, source: hyper::Error) -> HttpError {
        HttpError::UnixExchange { socket: self.socket.clone(), source }
    }
}

#[cfg(test)]
mod tests;
