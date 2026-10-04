//! Redaction of headers and URLs before they reach a log, an error or a transcript.
//!
//! The rules over-redact on purpose: a header or query parameter whose name merely
//! looks like it could carry a credential is hidden. A hidden harmless value costs a
//! little debugging; a logged token costs a credential.

use std::fmt;

use http::header::{AUTHORIZATION, PROXY_AUTHORIZATION};
use http::{HeaderMap, HeaderName, HeaderValue};
use url::Url;

/// What a redacted value is replaced with.
pub const REDACTED: &str = "[REDACTED]";

/// Header names hidden in full. They carry an identity rather than a credential.
const EXACT_HEADERS: &[&str] = &["chatgpt-account-id", "openai-organization", "openai-project"];

/// A header or query parameter whose lowercase name contains one of these is hidden.
const SENSITIVE_PARTS: &[&str] = &[
    "auth",
    "token",
    "secret",
    "password",
    "passwd",
    "cookie",
    "session",
    "key",
    "credential",
    "signature",
];

/// Query parameters of the OAuth flows, hidden although no pattern above matches them.
const EXACT_QUERY_KEYS: &[&str] = &["code", "state", "sig", "code_verifier"];

/// True when the value of the header `name` must not be logged as it is.
pub fn is_sensitive_header(name: &HeaderName) -> bool {
    let name = name.as_str();
    EXACT_HEADERS.contains(&name) || SENSITIVE_PARTS.iter().any(|part| name.contains(part))
}

/// The value to log for the header `name`.
///
/// A value that is not sensitive comes back unchanged. For `Authorization` and
/// `Proxy-Authorization` the scheme stays readable (`Bearer [REDACTED]`), because the
/// scheme is often what a debugging session needs. A value marked sensitive with
/// `HeaderValue::set_sensitive` is hidden whatever its name.
pub fn header_value(name: &HeaderName, value: &HeaderValue) -> HeaderValue {
    if !value.is_sensitive() && !is_sensitive_header(name) {
        return value.clone();
    }
    let scheme = (name == AUTHORIZATION || name == PROXY_AUTHORIZATION)
        .then(|| auth_scheme(value))
        .flatten();
    let text = match scheme {
        Some(scheme) => format!("{scheme} {REDACTED}"),
        None => REDACTED.to_owned(),
    };
    // NOTE: the replacement is printable ASCII, so the conversion cannot fail; the
    // fallback keeps that invariant out of a panic path.
    HeaderValue::from_str(&text).unwrap_or(HeaderValue::from_static(REDACTED))
}

/// A copy of `headers` with every sensitive value replaced, for a transcript.
pub fn header_map(headers: &HeaderMap) -> HeaderMap {
    let mut redacted = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        redacted.append(name.clone(), header_value(name, value));
    }
    redacted
}

/// A view of `headers` whose `Debug` and `Display` output is redacted, for a log
/// field. It allocates only when it is formatted.
pub fn headers(headers: &HeaderMap) -> RedactedHeaders<'_> {
    RedactedHeaders(headers)
}

/// `headers` with sensitive values hidden when formatted; see [`headers()`].
#[derive(Clone, Copy)]
pub struct RedactedHeaders<'a>(&'a HeaderMap);

impl fmt::Debug for RedactedHeaders<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for (name, value) in self.0 {
            map.entry(&name.as_str(), &header_value(name, value));
        }
        map.finish()
    }
}

impl fmt::Display for RedactedHeaders<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (name, value)) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            let value = header_value(name, value);
            write!(f, "{name}: {}", String::from_utf8_lossy(value.as_bytes()))?;
        }
        Ok(())
    }
}

/// `url` as text with its password, its fragment and the values of sensitive query
/// parameters replaced by [`REDACTED`]. Parameter names and other values keep their
/// original encoding, so the text stays recognisable.
pub fn url(url: &Url) -> String {
    let mut base = url.clone();
    if base.password().is_some() {
        // Only `cannot-be-a-base` URLs refuse a password change, and those have no
        // password to begin with.
        let _ = base.set_password(Some(REDACTED));
    }
    base.set_query(None);
    base.set_fragment(None);
    let mut text = String::from(base);
    if let Some(query) = url.query() {
        text.push('?');
        for (index, pair) in query.split('&').enumerate() {
            if index > 0 {
                text.push('&');
            }
            match pair.split_once('=') {
                Some((key, _)) if is_sensitive_query_key(key) => {
                    text.push_str(key);
                    text.push('=');
                    text.push_str(REDACTED);
                }
                _ => text.push_str(pair),
            }
        }
    }
    if url.fragment().is_some() {
        text.push('#');
        text.push_str(REDACTED);
    }
    text
}

fn is_sensitive_query_key(raw_key: &str) -> bool {
    let decoded: String = url::form_urlencoded::parse(raw_key.as_bytes())
        .next()
        .map(|(key, _)| key.to_lowercase())
        .unwrap_or_default();
    EXACT_QUERY_KEYS.contains(&decoded.as_str())
        || SENSITIVE_PARTS.iter().any(|part| decoded.contains(part))
}

/// The scheme of an `Authorization` value when it has one, such as `Bearer`. Only a
/// short word of letters counts, so a bare key with a space in it is never shown.
fn auth_scheme(value: &HeaderValue) -> Option<&str> {
    let text = value.to_str().ok()?;
    let (scheme, rest) = text.split_once(' ')?;
    let plausible = !scheme.is_empty()
        && !rest.is_empty()
        && scheme.len() <= 16
        && scheme.bytes().all(|b| b.is_ascii_alphabetic());
    plausible.then_some(scheme)
}

#[cfg(test)]
mod tests;
