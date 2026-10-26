//! The opening handshake of the WebSocket listener (RFC 6455, section 4.2), as pure
//! functions over bytes.
//!
//! [`parse`] reads a request head of at most [`MAX_HEAD_BYTES`] bytes and
//! [`MAX_HEADERS`] headers. It accepts only `GET /` over HTTP/1.1 with the headers of a
//! WebSocket upgrade, version 13, a key of 16 bytes and either no `Origin` or one of
//! the origins the caller allows. Any other request gets a plain HTTP error from
//! [`refusal_response`], never a 200.
//!
//! The parser never reads a `Cookie` or `Authorization` header and never copies a
//! header value into a refusal, so nothing a client sends reaches a log line. A query
//! string is refused with the path, so no credential can come in the URL.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha1::{Digest as _, Sha1};

/// The largest request head, with its final empty line.
pub(crate) const MAX_HEAD_BYTES: usize = 8 * 1024;

/// The most headers a request head may have.
pub(crate) const MAX_HEADERS: usize = 32;

/// The GUID that RFC 6455 (section 1.3) appends to a key to make the accept value.
const ACCEPT_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The only WebSocket version the listener speaks.
const VERSION: &str = "13";

/// Why the listener refused a handshake. Each reason has its HTTP status; the response
/// carries a short text that names the rule and nothing that the client sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WebSocketRefusal {
    /// The request head is larger than 8 KiB.
    HeadTooLarge,
    /// The request head has more than 32 headers.
    TooManyHeaders,
    /// The bytes are not an HTTP request head, a header value is not UTF-8, or a header
    /// that must come once (`Host`, `Origin`, `Sec-WebSocket-Key`,
    /// `Sec-WebSocket-Version`) is missing or repeated.
    Malformed,
    /// The request is not HTTP/1.1.
    HttpVersion,
    /// The method is not `GET`.
    Method,
    /// The target is not `/`, or it has a query string.
    Path,
    /// The request has a body (`Content-Length` other than 0, or `Transfer-Encoding`).
    Body,
    /// The request does not ask for a WebSocket upgrade (`Upgrade: websocket` and
    /// `Connection: upgrade`).
    NotUpgrade,
    /// The WebSocket version is not 13.
    WebSocketVersion,
    /// The `Sec-WebSocket-Key` is not 16 bytes in base64.
    Key,
    /// The `Origin` is not one that the listener allows.
    Origin,
}

impl WebSocketRefusal {
    /// The HTTP status of the response.
    pub const fn status(self) -> u16 {
        match self {
            WebSocketRefusal::HeadTooLarge | WebSocketRefusal::TooManyHeaders => 431,
            WebSocketRefusal::Malformed | WebSocketRefusal::Body | WebSocketRefusal::Key => 400,
            WebSocketRefusal::HttpVersion => 505,
            WebSocketRefusal::Method => 405,
            WebSocketRefusal::Path => 404,
            WebSocketRefusal::NotUpgrade | WebSocketRefusal::WebSocketVersion => 426,
            WebSocketRefusal::Origin => 403,
        }
    }

    /// The reason phrase of the status line.
    const fn phrase(self) -> &'static str {
        match self.status() {
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            426 => "Upgrade Required",
            431 => "Request Header Fields Too Large",
            _ => "HTTP Version Not Supported",
        }
    }

    /// The headers that the status needs besides the fixed ones, each with its CRLF.
    const fn extra_headers(self) -> &'static str {
        match self {
            WebSocketRefusal::Method => "Allow: GET\r\n",
            WebSocketRefusal::NotUpgrade => "Upgrade: websocket\r\n",
            WebSocketRefusal::WebSocketVersion => "Sec-WebSocket-Version: 13\r\n",
            _ => "",
        }
    }

    /// One sentence for the response body and the logs.
    const fn text(self) -> &'static str {
        match self {
            WebSocketRefusal::HeadTooLarge => "the request head is larger than 8 KiB",
            WebSocketRefusal::TooManyHeaders => "the request head has more than 32 headers",
            WebSocketRefusal::Malformed => "the request head is not valid",
            WebSocketRefusal::HttpVersion => "only HTTP/1.1 is served",
            WebSocketRefusal::Method => "only GET is served",
            WebSocketRefusal::Path => "only the path / is served",
            WebSocketRefusal::Body => "the request must not have a body",
            WebSocketRefusal::NotUpgrade => "only WebSocket upgrades are served",
            WebSocketRefusal::WebSocketVersion => "only WebSocket version 13 is served",
            WebSocketRefusal::Key => "the WebSocket key is not valid",
            WebSocketRefusal::Origin => "the origin is not allowed",
        }
    }
}

impl fmt::Display for WebSocketRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// What [`parse`] found in the bytes so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Parsed {
    /// The head is not complete yet; read more.
    Partial,
    /// A valid upgrade request.
    Complete(Upgrade),
}

/// A valid upgrade request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Upgrade {
    /// The length of the head with its final empty line. The bytes after it are the
    /// first WebSocket frames.
    pub(crate) head_len: usize,
    /// The `Sec-WebSocket-Accept` value for the request's key.
    pub(crate) accept: String,
}

/// Parses the request head at the start of `buf` and checks it, allowing a missing
/// `Origin` or one equal to an entry of `origins`.
///
/// `buf` holds every byte read so far. The caller reads at most [`MAX_HEAD_BYTES`], so
/// a head that is still partial at that length is too large.
pub(crate) fn parse(buf: &[u8], origins: &[String]) -> Result<Parsed, WebSocketRefusal> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    let head_len = match request.parse(buf) {
        Ok(httparse::Status::Complete(len)) if len <= MAX_HEAD_BYTES => len,
        Ok(httparse::Status::Complete(_)) => return Err(WebSocketRefusal::HeadTooLarge),
        Ok(httparse::Status::Partial) if buf.len() >= MAX_HEAD_BYTES => {
            return Err(WebSocketRefusal::HeadTooLarge);
        }
        Ok(httparse::Status::Partial) => return Ok(Parsed::Partial),
        Err(httparse::Error::TooManyHeaders) => return Err(WebSocketRefusal::TooManyHeaders),
        Err(_) => return Err(WebSocketRefusal::Malformed),
    };
    let key = check(&request, origins)?;
    Ok(Parsed::Complete(Upgrade { head_len, accept: accept_value(key) }))
}

/// The headers of a request head that the handshake reads.
#[derive(Debug, Default)]
struct Headers<'a> {
    host: Once<'a>,
    origin: Once<'a>,
    key: Once<'a>,
    version: Once<'a>,
    upgrade_websocket: bool,
    connection_upgrade: bool,
    body: bool,
}

/// A header that must come at most once.
#[derive(Debug, Default)]
enum Once<'a> {
    #[default]
    Missing,
    Value(&'a str),
    Repeated,
}

impl<'a> Once<'a> {
    fn add(&mut self, value: &'a str) {
        *self = match self {
            Once::Missing => Once::Value(value),
            Once::Value(_) | Once::Repeated => Once::Repeated,
        };
    }

    /// The value; a repeated header is malformed.
    fn get(&self) -> Result<Option<&'a str>, WebSocketRefusal> {
        match self {
            Once::Missing => Ok(None),
            Once::Value(value) => Ok(Some(value)),
            Once::Repeated => Err(WebSocketRefusal::Malformed),
        }
    }
}

/// Checks a complete request head and returns its key.
fn check<'a>(
    request: &httparse::Request<'_, 'a>,
    origins: &[String],
) -> Result<&'a str, WebSocketRefusal> {
    if request.version != Some(1) {
        return Err(WebSocketRefusal::HttpVersion);
    }
    if request.method != Some("GET") {
        return Err(WebSocketRefusal::Method);
    }
    if request.path != Some("/") {
        return Err(WebSocketRefusal::Path);
    }
    let headers = read_headers(request.headers)?;
    // NOTE: the origin is checked before the upgrade headers, so a web page that tries
    // the socket learns only that its origin is refused.
    if let Some(origin) = headers.origin.get()?
        && !origins.iter().any(|allowed| allowed == origin)
    {
        return Err(WebSocketRefusal::Origin);
    }
    if headers.host.get()?.is_none() {
        return Err(WebSocketRefusal::Malformed);
    }
    if headers.body {
        return Err(WebSocketRefusal::Body);
    }
    if !(headers.upgrade_websocket && headers.connection_upgrade) {
        return Err(WebSocketRefusal::NotUpgrade);
    }
    match headers.version.get()? {
        Some(VERSION) => {}
        Some(_) => return Err(WebSocketRefusal::WebSocketVersion),
        None => return Err(WebSocketRefusal::Malformed),
    }
    let Some(key) = headers.key.get()? else {
        return Err(WebSocketRefusal::Malformed);
    };
    match STANDARD.decode(key) {
        Ok(bytes) if bytes.len() == 16 => Ok(key),
        _ => Err(WebSocketRefusal::Key),
    }
}

/// Sorts the headers that the handshake reads. A `Cookie`, an `Authorization` and every
/// other header are skipped unread.
fn read_headers<'a>(raw: &[httparse::Header<'a>]) -> Result<Headers<'a>, WebSocketRefusal> {
    let mut headers = Headers::default();
    for header in raw {
        let name = header.name;
        let is = |wanted: &str| name.eq_ignore_ascii_case(wanted);
        let interesting = is("host")
            || is("origin")
            || is("sec-websocket-key")
            || is("sec-websocket-version")
            || is("upgrade")
            || is("connection")
            || is("content-length")
            || is("transfer-encoding");
        if !interesting {
            continue;
        }
        let value = std::str::from_utf8(header.value)
            .map_err(|_| WebSocketRefusal::Malformed)?
            .trim_matches([' ', '\t']);
        if is("host") {
            headers.host.add(value);
        } else if is("origin") {
            headers.origin.add(value);
        } else if is("sec-websocket-key") {
            headers.key.add(value);
        } else if is("sec-websocket-version") {
            headers.version.add(value);
        } else if is("upgrade") {
            headers.upgrade_websocket |= has_token(value, "websocket");
        } else if is("connection") {
            headers.connection_upgrade |= has_token(value, "upgrade");
        } else if is("content-length") {
            headers.body |= value != "0";
        } else {
            headers.body = true;
        }
    }
    Ok(headers)
}

/// True when the comma-separated list `value` holds `token`, in any case.
fn has_token(value: &str, token: &str) -> bool {
    value.split(',').any(|item| item.trim_matches([' ', '\t']).eq_ignore_ascii_case(token))
}

/// The `Sec-WebSocket-Accept` value for `key` (RFC 6455, section 4.2.2): the SHA-1 of
/// the key and a fixed GUID, in base64.
pub(crate) fn accept_value(key: &str) -> String {
    let mut hash = Sha1::new();
    hash.update(key.as_bytes());
    hash.update(ACCEPT_GUID.as_bytes());
    STANDARD.encode(hash.finalize())
}

/// The `101 Switching Protocols` answer to a valid upgrade. It offers no extension and
/// no subprotocol.
pub(crate) fn accept_response(upgrade: &Upgrade) -> Vec<u8> {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\
         \r\n",
        upgrade.accept
    )
    .into_bytes()
}

/// The plain HTTP error for `refusal`. The connection closes after it.
pub(crate) fn refusal_response(refusal: WebSocketRefusal) -> Vec<u8> {
    let body = format!("{}\n", refusal.text());
    format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         {}\
         \r\n\
         {body}",
        refusal.status(),
        refusal.phrase(),
        body.len(),
        refusal.extra_headers(),
    )
    .into_bytes()
}

#[cfg(test)]
mod tests;
