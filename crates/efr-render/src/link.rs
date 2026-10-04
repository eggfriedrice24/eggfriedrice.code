//! Hyperlink targets: which link destinations, bare URLs and inline code become OSC 8
//! hyperlinks, and how a target is encoded so that nothing in it can end the escape
//! sequence early or smuggle in another one.

use std::fmt::Write as _;
use std::ops::Range;

/// Schemes a terminal may open. Anything else (`javascript:`, a custom scheme) is
/// shown as text, never made clickable.
const SCHEMES: &[&str] = &["https://", "http://", "mailto:", "file://"];

/// The OSC 8 target for a markdown link destination: a URL with an allowed scheme, or
/// an absolute file path as a `file://` URL. A relative path has no target, because
/// this crate does not know the directory it is relative to.
pub(crate) fn link_target(destination: &str) -> Option<String> {
    let destination = destination.trim();
    if has_scheme(destination) {
        return Some(encode(destination));
    }
    if destination.starts_with('/') && !destination.starts_with("//") {
        return Some(file_url(destination));
    }
    None
}

/// The target for an inline code span that is a whole URL or an absolute path with at
/// least two components (so `/help` and other slash commands stay plain).
pub(crate) fn code_target(code: &str) -> Option<String> {
    if code.is_empty() || code.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    if code.starts_with("https://") || code.starts_with("http://") {
        return Some(encode(code));
    }
    let looks_like_path = code.starts_with('/')
        && !code.starts_with("//")
        && code[1..].trim_end_matches('/').contains('/');
    looks_like_path.then(|| file_url(code))
}

fn has_scheme(destination: &str) -> bool {
    SCHEMES.iter().any(|scheme| {
        destination.len() > scheme.len() && destination[..scheme.len()].eq_ignore_ascii_case(scheme)
    })
}

/// A `file://` URL for an absolute path. A trailing `:line` or `:line:column`, as
/// compilers and models write it, is not part of the file name and is left out.
pub(crate) fn file_url(path: &str) -> String {
    let path = strip_line_suffix(path);
    let mut url = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

fn strip_line_suffix(path: &str) -> &str {
    let mut rest = path;
    for _ in 0..2 {
        match rest.rsplit_once(':') {
            Some((head, digits))
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                rest = head;
            }
            _ => break,
        }
    }
    rest
}

/// Percent-encodes every byte outside printable ASCII. ESC, BEL and spaces in a target
/// would end the OSC 8 sequence; this leaves only characters that cannot.
pub(crate) fn encode(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for byte in url.bytes() {
        if (0x21..=0x7e).contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Byte ranges of bare `http://` and `https://` URLs in prose. A URL ends at a space
/// or at a character that cannot be part of one; trailing punctuation and an
/// unbalanced closing bracket belong to the sentence, not the URL.
pub(crate) fn find_urls(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find("http") {
        let start = from + offset;
        let rest = &text[start..];
        let scheme_len = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            from = start + 4;
            continue;
        };
        let at_boundary = text[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let end = start
            + rest
                .find(|c: char| c.is_whitespace() || c.is_control() || "<>\"`".contains(c))
                .unwrap_or(rest.len());
        let end = trim_url_end(text, start, end);
        if at_boundary && end > start + scheme_len {
            found.push(start..end);
            from = end;
        } else {
            from = start + scheme_len;
        }
    }
    found
}

fn trim_url_end(text: &str, start: usize, mut end: usize) -> usize {
    loop {
        let Some(last) = text[start..end].chars().next_back() else { return end };
        let unbalanced = |open: char| {
            let url = &text[start..end];
            url.matches(open).count() < url.matches(last).count()
        };
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_' => true,
            ')' => unbalanced('('),
            ']' => unbalanced('['),
            '}' => unbalanced('{'),
            _ => false,
        };
        if !drop {
            return end;
        }
        end -= last.len_utf8();
    }
}

#[cfg(test)]
mod tests;
