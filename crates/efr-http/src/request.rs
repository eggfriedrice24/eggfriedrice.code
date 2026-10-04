//! A request, built before it is sent.

use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Serialize;
use url::Url;
use zeroize::Zeroizing;

use crate::{HttpError, redact};

/// One HTTP request: method, URL, headers and a body held in memory.
///
/// The same type goes to [`HttpClient`](crate::HttpClient) and to the Unix-socket
/// client. It is cheap to clone (the body is shared), so a retry sends the same
/// request again. Its `Debug` output shows the redacted URL and
/// headers and the body length, never the body, which may hold a refresh token.
#[derive(Clone)]
pub struct HttpRequest {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Bytes,
    timeout: Option<Duration>,
    recorded: bool,
    idempotent: bool,
}

impl HttpRequest {
    /// A request with an empty body. `url` must be an absolute `http` or `https` URL.
    pub fn new(method: Method, url: &str) -> Result<Self, HttpError> {
        let url = Url::parse(url).map_err(|source| HttpError::InvalidUrl { source })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(HttpError::UnsupportedScheme { scheme: url.scheme().to_owned() });
        }
        Ok(HttpRequest {
            method,
            url,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            timeout: None,
            recorded: false,
            idempotent: false,
        })
    }

    /// A `GET` request.
    pub fn get(url: &str) -> Result<Self, HttpError> {
        HttpRequest::new(Method::GET, url)
    }

    /// A `POST` request.
    pub fn post(url: &str) -> Result<Self, HttpError> {
        HttpRequest::new(Method::POST, url)
    }

    /// Sets the header `name` to `value`, replacing any earlier value.
    #[must_use]
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Sets the header `name` to the text `value`. Fails when `value` holds bytes that
    /// a header cannot, such as a newline.
    pub fn header_text(self, name: HeaderName, value: &str) -> Result<Self, HttpError> {
        let value = HeaderValue::from_str(value)
            .map_err(|_| HttpError::InvalidHeaderValue { name: name.clone() })?;
        Ok(self.header(name, value))
    }

    /// Sets `Authorization: Bearer <token>`. The header is marked sensitive, so
    /// [`redact`] hides it whatever its name.
    pub fn bearer_auth(self, token: &SecretString) -> Result<Self, HttpError> {
        let mut text = Zeroizing::new(String::with_capacity(token.expose_secret().len() + 7));
        text.push_str("Bearer ");
        text.push_str(token.expose_secret());
        let mut value = HeaderValue::from_str(&text)
            .map_err(|_| HttpError::InvalidHeaderValue { name: AUTHORIZATION })?;
        value.set_sensitive(true);
        Ok(self.header(AUTHORIZATION, value))
    }

    /// Sets the body to `body` as JSON, with `Content-Type: application/json`.
    pub fn json<T: Serialize + ?Sized>(self, body: &T) -> Result<Self, HttpError> {
        let bytes = serde_json::to_vec(body).map_err(|source| HttpError::EncodeJson { source })?;
        Ok(self.body(HeaderValue::from_static("application/json"), bytes))
    }

    /// Sets the body to `pairs` form-encoded, with
    /// `Content-Type: application/x-www-form-urlencoded`, as an OAuth token endpoint
    /// expects.
    #[must_use]
    pub fn form(self, pairs: &[(&str, &str)]) -> Self {
        let encoded =
            url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        self.body(HeaderValue::from_static("application/x-www-form-urlencoded"), encoded)
    }

    /// Sets the body and its content type.
    #[must_use]
    pub fn body(mut self, content_type: HeaderValue, body: impl Into<Bytes>) -> Self {
        self.headers.insert(CONTENT_TYPE, content_type);
        self.body = body.into();
        self
    }

    /// A deadline for the whole exchange, body included. Leave it unset for a stream
    /// that may run for minutes; the client's read timeout still catches a stall.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Passes this exchange to the client's [`Recorder`](crate::Recorder). Recording is
    /// opt-in per request, so a token exchange is never written to a transcript by
    /// accident.
    #[must_use]
    pub fn recorded(mut self) -> Self {
        self.recorded = true;
        self
    }

    /// Marks the request idempotent: two copies of it have the same effect on the server
    /// as one. [`HttpClient::send_with_retry`](crate::HttpClient::send_with_retry) then
    /// sends it again after a failure that may have reached the server, such as a
    /// timeout or a 502. A `POST` is never marked by default.
    #[must_use]
    pub fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }

    /// True when the request is marked [`idempotent`](HttpRequest::idempotent) or its
    /// method is idempotent by RFC 9110 (`GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT`,
    /// `DELETE`).
    pub fn is_idempotent(&self) -> bool {
        self.idempotent || self.method.is_idempotent()
    }

    /// The method.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// The URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The body.
    pub fn body_bytes(&self) -> &Bytes {
        &self.body
    }

    /// The deadline for the whole exchange, when one is set.
    pub fn deadline(&self) -> Option<Duration> {
        self.timeout
    }

    /// True when the exchange goes to the recorder.
    pub fn is_recorded(&self) -> bool {
        self.recorded
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &redact::url(&self.url))
            .field("headers", &redact::headers(&self.headers))
            .field("body_len", &self.body.len())
            .field("timeout", &self.timeout)
            .field("recorded", &self.recorded)
            .field("idempotent", &self.idempotent)
            .finish()
    }
}

#[cfg(test)]
mod tests;
