//! The TLS ClientHello parser (DP6): the server name of the first bytes of a tunnel.
//!
//! The proxy terminates no TLS. It reads the client's first handshake message to learn
//! the server name (SNI) that the client asks for, and whether the hello asks for
//! Encrypted Client Hello. The parser:
//!
//! - reads TLS records of type 22 (handshake) with the version byte `3` and a minor
//!   version of at most 4, each 1 to 16384 bytes long, and joins their fragments;
//! - needs a handshake message of type 1 (ClientHello);
//! - walks the hello with every length checked against its parent: the session id,
//!   the cipher suites, the compression methods and the extensions, which must fill
//!   the rest exactly, each type at most once;
//! - takes the `host_name` entry of the `server_name` extension (type 0);
//! - marks the extension types `0xfe08` to `0xfe0d` (the drafts and the final code of
//!   Encrypted Client Hello) and `0xffce` (encrypted SNI) as `ech` of [`Hello::Done`].
//!
//! It never panics and never reads past its slice. A byte that cannot start a TLS
//! record gives [`Hello::NotTls`] at once, so a plain HTTP request or an ssh banner is
//! cut without a wait.

/// The largest fragment of one TLS record (RFC 8446, section 5.1).
const MAX_FRAGMENT: usize = 16384;

/// What the parser made of the first bytes of a tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hello {
    /// A whole ClientHello.
    Done {
        /// The `host_name` of the `server_name` extension, as the client wrote it.
        sni: Option<String>,
        /// The hello has an Encrypted Client Hello or encrypted SNI extension.
        ech: bool,
        /// The bytes up to the end of the record that completed the hello.
        used: usize,
    },
    /// No verdict yet: the parser needs more bytes.
    Partial,
    /// The bytes are no TLS handshake records, or the hello inside them is malformed.
    NotTls,
    /// The first handshake message is not a ClientHello.
    NotHello,
}

/// True for the extension types of Encrypted Client Hello and encrypted SNI.
pub fn is_ech(kind: u16) -> bool {
    (0xfe08..=0xfe0d).contains(&kind) || kind == 0xffce
}

/// Reads the ClientHello at the start of `bytes`, joined over its records. Never reads
/// past `bytes`.
pub fn parse(bytes: &[u8]) -> Hello {
    let mut handshake = Vec::new();
    let mut at = 0;
    loop {
        let header = &bytes[at.min(bytes.len())..bytes.len().min(at + 5)];
        if header.first().is_some_and(|kind| *kind != 22)
            || header.get(1).is_some_and(|major| *major != 3)
            || header.get(2).is_some_and(|minor| *minor > 4)
        {
            return Hello::NotTls;
        }
        let [_, _, _, high, low] = *header else {
            return Hello::Partial;
        };
        let length = usize::from(u16::from_be_bytes([high, low]));
        if length == 0 || length > MAX_FRAGMENT {
            return Hello::NotTls;
        }
        let Some(fragment) = bytes.get(at + 5..at + 5 + length) else {
            return Hello::Partial;
        };
        handshake.extend_from_slice(fragment);
        at += 5 + length;
        if handshake.first().is_some_and(|kind| *kind != 1) {
            return Hello::NotHello;
        }
        let mut reader = Reader(&handshake);
        let (Some(_), Some(size)) = (reader.u8(), reader.u24()) else {
            continue;
        };
        if let Some(body) = reader.take(size) {
            return match client_hello(body) {
                Some((sni, ech)) => Hello::Done { sni, ech, used: at },
                None => Hello::NotTls,
            };
        }
    }
}

/// The server name and the ECH mark of a ClientHello body; `None` when it is
/// malformed.
fn client_hello(body: &[u8]) -> Option<(Option<String>, bool)> {
    let mut reader = Reader(body);
    let version = reader.take(2)?;
    if version[0] != 3 {
        return None;
    }
    reader.take(32)?;
    let session = reader.u8()?;
    if session > 32 {
        return None;
    }
    reader.take(usize::from(session))?;
    let suites = reader.u16()?;
    if suites < 2 || suites % 2 != 0 {
        return None;
    }
    reader.take(usize::from(suites))?;
    let compression = reader.u8()?;
    if compression == 0 {
        return None;
    }
    reader.take(usize::from(compression))?;
    if reader.0.is_empty() {
        return Some((None, false));
    }
    let size = reader.u16()?;
    let mut extensions = Reader(reader.take(usize::from(size))?);
    if !reader.0.is_empty() {
        return None;
    }
    let mut seen = Vec::new();
    let mut sni = None;
    let mut ech = false;
    while !extensions.0.is_empty() {
        let kind = extensions.u16()?;
        let size = extensions.u16()?;
        let data = extensions.take(usize::from(size))?;
        if seen.contains(&kind) {
            return None;
        }
        seen.push(kind);
        ech |= is_ech(kind);
        if kind == 0 {
            sni = server_name(data)?;
        }
    }
    Some((sni, ech))
}

/// The `host_name` of a `server_name` extension; `Some(None)` for a list without one,
/// `None` when the extension is malformed.
fn server_name(data: &[u8]) -> Option<Option<String>> {
    let mut reader = Reader(data);
    let size = reader.u16()?;
    let mut list = Reader(reader.take(usize::from(size))?);
    if !reader.0.is_empty() {
        return None;
    }
    let mut name = None;
    while !list.0.is_empty() {
        let kind = list.u8()?;
        let size = list.u16()?;
        let entry = list.take(usize::from(size))?;
        if kind != 0 {
            continue;
        }
        if name.is_some() || entry.is_empty() || !entry.iter().all(|byte| byte.is_ascii_graphic()) {
            return None;
        }
        name = Some(String::from_utf8(entry.to_vec()).ok()?);
    }
    Some(name)
}

/// A cursor over bytes; each read is `None` when too few bytes are left.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if count > self.0.len() {
            return None;
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u24(&mut self) -> Option<usize> {
        self.take(3).map(|bytes| {
            usize::from(bytes[0]) << 16 | usize::from(bytes[1]) << 8 | usize::from(bytes[2])
        })
    }
}

#[cfg(test)]
mod tests;
