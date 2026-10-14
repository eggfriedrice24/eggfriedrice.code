//! The request head parser (DP6, DP8): the first bytes that a client of the proxy
//! sends, either a `CONNECT host:port` for a tunnel or an absolute-form `GET` or `HEAD`
//! for plain HTTP.
//!
//! The parser reads a strict subset of HTTP/1.1. Every head that it does not
//! understand is refused, never guessed:
//!
//! - At most `limit` bytes, up to the first `CRLF CRLF`. Lines end in `CRLF` only.
//! - Visible ASCII and the space only: a control character, a lone `CR` or `LF`, a tab
//!   or a byte over `0x7e` is a refusal.
//! - The request line is `METHOD SP target SP HTTP/1.1` (or `HTTP/1.0`), with one space
//!   between the parts.
//! - A header line is `name: value`. A folded line (obs-fold), a name that is no token,
//!   and a second `Host`, `Content-Length` or `Transfer-Encoding` are refusals.
//! - `CONNECT` takes the authority form `host:port`, with the port. `GET` and `HEAD`
//!   take the absolute form `http://host[:port]/path`: no user info, no `%` or brackets
//!   in a name, no fragment.
//! - A `Host` header must name the same host and port as the target.
//! - `GET`, `HEAD` and `CONNECT` have no body: a `Content-Length` other than `0` or a
//!   `Transfer-Encoding` is a refusal.
//!
//! The proxy never forwards the client's head: [`upstream_head`] writes a new one from
//! the parsed values and the headers of [`FORWARDED`].

use efr_permissions::network::host_and_port;
use efr_protocol::BlockReason;

/// The client headers that a plain request keeps (DP8). Every other header, also
/// `Proxy-Authorization` and `Cookie`, stays with the proxy.
pub const FORWARDED: [&str; 9] = [
    "Accept",
    "Accept-Encoding",
    "Accept-Language",
    "User-Agent",
    "Range",
    "If-Modified-Since",
    "If-None-Match",
    "If-Range",
    "Cache-Control",
];

/// A request head that the parser read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// `CONNECT host:port`: a tunnel.
    Connect {
        /// The host in lower case with no trailing dot; an IPv6 address without its
        /// brackets.
        host: String,
        /// The port of the target.
        port: u16,
    },
    /// An absolute-form request for plain HTTP.
    Plain {
        /// The method.
        method: Method,
        /// The host of the target, as for [`Request::Connect`].
        host: String,
        /// The port of the target; 80 when it names none.
        port: u16,
        /// The origin-form target that the upstream gets: the path and the query.
        target: String,
        /// The headers of [`FORWARDED`] that the client sent, with the name as that
        /// list writes it.
        headers: Vec<(String, String)>,
    },
}

/// The method of a plain request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `HEAD`.
    Head,
    /// Any other method, which the proxy refuses with 405.
    Other(String),
}

impl Method {
    /// The method as the request line writes it.
    pub fn as_str(&self) -> &str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Other(other) => other,
        }
    }
}

/// What the parser made of the bytes so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed<T> {
    /// A whole head: the value, and the bytes up to and with the `CRLF CRLF`.
    Done {
        /// The value.
        value: T,
        /// The bytes that the head used.
        used: usize,
    },
    /// No refusal yet, and no end of the head: the parser needs more bytes.
    Partial,
    /// The head breaks a rule. The reason is [`BlockReason::BadRequest`].
    Refused(BlockReason),
}

/// Reads one request head from the start of `bytes`, at most `limit` bytes of it.
/// Bytes after the head stay for the caller (`used` says where the head ends).
pub fn parse(bytes: &[u8], limit: usize) -> Parsed<Request> {
    let window = &bytes[..bytes.len().min(limit)];
    let end = window.windows(4).position(|four| four == b"\r\n\r\n").map(|at| at + 4);
    let seen = end.map_or(window, |end| &window[..end]);
    if !clean(seen) {
        return Parsed::Refused(BlockReason::BadRequest);
    }
    let Some(end) = end else {
        return if bytes.len() >= limit {
            Parsed::Refused(BlockReason::BadRequest)
        } else {
            Parsed::Partial
        };
    };
    let Ok(text) = std::str::from_utf8(&window[..end - 4]) else {
        return Parsed::Refused(BlockReason::BadRequest);
    };
    match request(text) {
        Some(value) => Parsed::Done { value, used: end },
        None => Parsed::Refused(BlockReason::BadRequest),
    }
}

/// The head that the proxy sends upstream for a plain request (DP8): the method, the
/// origin-form target, `Host`, `Connection: close` and the kept headers. Empty for a
/// `CONNECT`, which sends no head upstream.
pub fn upstream_head(request: &Request) -> Vec<u8> {
    let Request::Plain { method, host, port, target, headers } = request else {
        return Vec::new();
    };
    let host = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    let authority = if *port == 80 { host } else { format!("{host}:{port}") };
    let mut head = format!(
        "{} {target} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n",
        method.as_str()
    );
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    head.into_bytes()
}

/// True when `bytes` holds only visible ASCII, the space and `CRLF` pairs. A `CR` at
/// the very end may still get its `LF`.
fn clean(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().all(|(at, byte)| match byte {
        b'\r' => bytes.get(at + 1).is_none_or(|next| *next == b'\n'),
        b'\n' => at > 0 && bytes[at - 1] == b'\r',
        byte => (b' '..=b'~').contains(byte),
    })
}

/// The request of a head without its final `CRLF CRLF`; `None` for any refusal.
fn request(text: &str) -> Option<Request> {
    let mut lines = text.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !is_method(method) || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return None;
    }
    let headers = Headers::read(lines)?;
    let bodyless = matches!(method, "GET" | "HEAD" | "CONNECT");
    if bodyless
        && (headers.transfer_encoding || headers.content_length.is_some_and(|length| length != "0"))
    {
        return None;
    }
    if method == "CONNECT" {
        let (host, port) = connect_target(target)?;
        if let Some(header) = headers.host {
            let (named, named_port) = authority(header)?;
            if named != host || named_port.is_some_and(|named_port| named_port != port) {
                return None;
            }
        }
        return Some(Request::Connect { host, port });
    }
    let (host, port, path) = absolute_target(target)?;
    if let Some(header) = headers.host {
        let (named, named_port) = authority(header)?;
        if named != host || named_port.unwrap_or(80) != port {
            return None;
        }
    }
    let method = match method {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        other => Method::Other(other.to_owned()),
    };
    Some(Request::Plain { method, host, port, target: path, headers: headers.kept })
}

/// The headers that the parser looks at.
#[derive(Default)]
struct Headers<'a> {
    host: Option<&'a str>,
    content_length: Option<&'a str>,
    transfer_encoding: bool,
    kept: Vec<(String, String)>,
}

impl<'a> Headers<'a> {
    fn read(lines: impl Iterator<Item = &'a str>) -> Option<Headers<'a>> {
        let mut headers = Headers::default();
        for line in lines {
            let (name, value) = line.split_once(':')?;
            if name.is_empty() || !name.bytes().all(is_token_byte) {
                return None;
            }
            let value = value.trim_matches(' ');
            if name.eq_ignore_ascii_case("host") {
                if headers.host.replace(value).is_some() {
                    return None;
                }
            } else if name.eq_ignore_ascii_case("content-length") {
                if headers.content_length.replace(value).is_some() {
                    return None;
                }
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                if headers.transfer_encoding {
                    return None;
                }
                headers.transfer_encoding = true;
            } else if let Some(kept) = FORWARDED.iter().find(|kept| kept.eq_ignore_ascii_case(name))
            {
                headers.kept.push(((*kept).to_owned(), value.to_owned()));
            }
        }
        Some(headers)
    }
}

/// `host:port` of a `CONNECT`, with the port.
fn connect_target(target: &str) -> Option<(String, u16)> {
    let (host, port) = authority(target)?;
    Some((host, port?))
}

/// `http://host[:port][/path][?query]` as the host, the port (80 when absent) and the
/// origin-form target.
fn absolute_target(target: &str) -> Option<(String, u16, String)> {
    let scheme = target.get(..7)?;
    if !scheme.eq_ignore_ascii_case("http://") || target.contains('#') {
        return None;
    }
    let rest = &target[7..];
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority_text, path) = rest.split_at(split);
    let (host, port) = authority(authority_text)?;
    let path = match path {
        "" => "/".to_owned(),
        path if path.starts_with('?') => format!("/{path}"),
        path => path.to_owned(),
    };
    Some((host, port.unwrap_or(80), path))
}

/// An authority `host[:port]`: the host as [`host_and_port`] writes it and the port when
/// the text names one. User info, `%`, a path and brackets anywhere but around a whole
/// IPv6 address are refusals.
fn authority(text: &str) -> Option<(String, Option<u16>)> {
    if text.is_empty() || text.contains(['@', '%', '/', '?', '#', ' ']) {
        return None;
    }
    let (host, port) = if let Some(bracketed) = text.strip_prefix('[') {
        let (address, after) = bracketed.split_once(']')?;
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':')?),
        };
        if address.contains(['[', ']']) {
            return None;
        }
        (&text[..address.len() + 2], port)
    } else {
        if text.contains(['[', ']']) {
            return None;
        }
        match text.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (text, None),
        }
    };
    let port = match port {
        Some(port) => Some(number(port)?),
        None => None,
    };
    // NOTE: host_and_port reads a word with no port as port 443, so the port it gives
    // back is ignored here; `number` read the real one.
    let (host, _) = host_and_port(host)?;
    Some((host, port))
}

/// A port of one to five digits, 1 to 65535. Rust's integer parser takes a `+`, which
/// no port has.
fn number(text: &str) -> Option<u16> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|port| *port != 0)
}

/// A method of upper-case letters, 1 to 16 of them.
fn is_method(method: &str) -> bool {
    (1..=16).contains(&method.len()) && method.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// A byte of an HTTP token (RFC 9110, section 5.6.2).
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[cfg(test)]
mod tests;
