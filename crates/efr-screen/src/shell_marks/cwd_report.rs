//! OSC 7, the working directory a shell reports after every `cd`.
//!
//! Two URL forms are in use. `file://host/path` is a URL: its path is
//! percent-encoded, and a query or fragment is not part of it. kitty's
//! `kitty-shell-cwd://host/path`, which ghostty's zsh integration emits, carries the
//! path raw, so nothing in it is decoded or cut. The scheme is matched without regard
//! to case, as URL schemes are.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

use super::percent_decode;

/// A parsed OSC 7 report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CwdReport {
    /// The host part of the URL; `None` when it is empty (`file:///tmp`).
    pub(crate) host: Option<String>,
    /// The absolute path. It may hold bytes that are not UTF-8.
    pub(crate) path: PathBuf,
}

/// Parses the URL after `7;`. `None` means it is not a report this scanner accepts:
/// an empty URL, another scheme, no path, a host that is not UTF-8 or a `file` path
/// with a broken percent escape.
pub(crate) fn parse(url: &[u8]) -> Option<CwdReport> {
    let (rest, raw) = if let Some(rest) = strip_scheme(url, b"file://") {
        (rest, false)
    } else {
        (strip_scheme(url, b"kitty-shell-cwd://")?, true)
    };
    let slash = rest.iter().position(|&byte| byte == b'/')?;
    let (host, path) = rest.split_at(slash);
    let host = match host {
        [] => None,
        host => Some(String::from_utf8(host.to_vec()).ok()?),
    };
    let path = if raw {
        path.to_vec()
    } else {
        let end = path.iter().position(|&byte| byte == b'?' || byte == b'#').unwrap_or(path.len());
        percent_decode(&path[..end])?
    };
    Some(CwdReport { host, path: PathBuf::from(OsString::from_vec(path)) })
}

fn strip_scheme<'a>(url: &'a [u8], scheme: &[u8]) -> Option<&'a [u8]> {
    let head = url.get(..scheme.len())?;
    head.eq_ignore_ascii_case(scheme).then(|| &url[scheme.len()..])
}

#[cfg(test)]
mod tests;
