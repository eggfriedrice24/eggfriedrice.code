//! The one outbound HTTPS client.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use futures::TryStreamExt as _;

use crate::recorder::{ExchangeId, Record, Recorder, record_body};
use crate::retry::HttpAttempt;
use crate::{ByteStream, HttpError, HttpRequest, HttpResponse, RetryPolicy, redact};

/// Settings for [`HttpClient::new`].
///
/// Start from [`HttpConfig::default`] and adjust the fields; the struct is
/// `#[non_exhaustive]` so that a later field does not break callers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HttpConfig {
    /// The `User-Agent` header.
    pub user_agent: String,
    /// The longest wait for a TCP and TLS connection.
    pub connect_timeout: Duration,
    /// The longest silence between two reads of a response. It catches a stalled
    /// stream without limiting how long a healthy one runs.
    pub read_timeout: Duration,
    /// How long an idle pooled connection is kept.
    pub pool_idle_timeout: Duration,
    /// The largest body that [`HttpResponse::bytes`] and friends read into memory.
    pub max_body_bytes: usize,
}

impl Default for HttpConfig {
    /// `efr/<version>`, 10 s to connect, 300 s of silence (a reasoning model can think
    /// for minutes between events), 90 s idle, 16 MiB bodies.
    fn default() -> Self {
        HttpConfig {
            user_agent: concat!("efr/", env!("CARGO_PKG_VERSION")).to_owned(),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(300),
            pool_idle_timeout: Duration::from_secs(90),
            max_body_bytes: 16 * 1024 * 1024,
        }
    }
}

/// The client for every outbound HTTP request of the daemon: reqwest over rustls (no
/// OpenSSL; `deny.toml` bans it), with the timeouts of [`HttpConfig`].
///
/// Clones share one connection pool, so the daemon builds one client and hands out
/// clones. Retries wait on the injected [`Clock`] and draw jitter from the injected
/// [`Rng`]. A [`Recorder`], when set, sees the requests marked
/// [`HttpRequest::recorded`].
#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    recorder: Option<Arc<dyn Recorder>>,
    next_exchange: Arc<AtomicU64>,
    max_body_bytes: usize,
}

impl HttpClient {
    /// Builds the client. Fails when reqwest cannot set up TLS.
    pub fn new(
        config: &HttpConfig,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Result<Self, HttpError> {
        let inner = reqwest::Client::builder()
            .tls_backend_rustls()
            .user_agent(config.user_agent.as_str())
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .pool_idle_timeout(config.pool_idle_timeout)
            .build()
            .map_err(|source| HttpError::BuildClient { source })?;
        Ok(HttpClient {
            inner,
            clock,
            rng,
            recorder: None,
            next_exchange: Arc::new(AtomicU64::new(1)),
            max_body_bytes: config.max_body_bytes,
        })
    }

    /// The same client with `recorder` receiving its recorded exchanges.
    #[must_use]
    pub fn with_recorder(mut self, recorder: Arc<dyn Recorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Sends `request` once. Any status is a response; only a failure to get one is
    /// an error.
    pub async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let exchange = ExchangeId(self.next_exchange.fetch_add(1, Ordering::Relaxed));
        let url = redact::url(request.url());
        let recorder = self.recorder.as_ref().filter(|_| request.is_recorded());
        if let Some(recorder) = recorder {
            recorder.record(&Record::Request {
                exchange,
                method: request.method(),
                url: &url,
                headers: &redact::header_map(request.headers()),
                body: request.body_bytes(),
            });
        }

        let mut builder = self
            .inner
            .request(request.method().clone(), request.url().clone())
            .headers(request.headers().clone())
            .body(request.body_bytes().clone());
        if let Some(deadline) = request.deadline() {
            builder = builder.timeout(deadline);
        }
        let response = match builder.send().await {
            Ok(response) => response,
            Err(source) => {
                let error = send_error(url, source);
                if let Some(recorder) = recorder {
                    recorder.record(&Record::Failed { exchange, error: &error });
                }
                return Err(error);
            }
        };

        let status = response.status();
        let headers = response.headers().clone();
        let body_url = url.clone();
        let mut body: ByteStream =
            Box::pin(response.bytes_stream().map_err(move |source| body_error(&body_url, source)));
        if let Some(recorder) = recorder {
            recorder.record(&Record::Response {
                exchange,
                status,
                headers: &redact::header_map(&headers),
            });
            body = record_body(body, Arc::clone(recorder), exchange);
        }
        Ok(HttpResponse::new(status, headers, url, body, self.max_body_bytes))
    }

    /// Sends `request` and sends it again while `policy` says so, waiting on the
    /// client's clock. Returns the last response or error.
    ///
    /// A request that is not [idempotent](HttpRequest::is_idempotent) is sent again
    /// only when the server certainly did not act on it (see [`HttpError::is_retryable`]
    /// and [`is_retryable_status`](crate::is_retryable_status)), so a `POST` never runs
    /// twice because a timeout hid its answer.
    pub async fn send_with_retry(
        &self,
        request: &HttpRequest,
        policy: &RetryPolicy,
    ) -> Result<HttpResponse, HttpError> {
        let idempotent = request.is_idempotent();
        policy
            .run(&*self.clock, &*self.rng, |_attempt| async move {
                match self.send(request).await {
                    Ok(value) => Ok(HttpAttempt { value, idempotent }),
                    Err(value) => Err(HttpAttempt { value, idempotent }),
                }
            })
            .await
            .map(|attempt| attempt.value)
            .map_err(|attempt| attempt.value)
    }
}

impl fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClient")
            .field("clock", &self.clock)
            .field("recorder", &self.recorder)
            .field("max_body_bytes", &self.max_body_bytes)
            .finish_non_exhaustive()
    }
}

/// Classifies a failure to get a response. The URL is dropped from the reqwest error
/// because it is unredacted; `url` is the redacted copy.
///
/// A connect error comes first, even when it is the connect timeout, because only it
/// proves that the request never left: a timeout of the whole exchange or of a read may
/// fire after the body was sent.
fn send_error(url: String, source: reqwest::Error) -> HttpError {
    let source = source.without_url();
    if source.is_connect() {
        HttpError::Connect { url, source }
    } else if source.is_timeout() {
        HttpError::Timeout { url }
    } else {
        HttpError::Send { url, source }
    }
}

fn body_error(url: &str, source: reqwest::Error) -> HttpError {
    let source = source.without_url();
    if source.is_timeout() {
        HttpError::Timeout { url: url.to_owned() }
    } else {
        HttpError::Body { url: url.to_owned(), source }
    }
}

#[cfg(test)]
mod tests;
