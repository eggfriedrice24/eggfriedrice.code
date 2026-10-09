//! The password in the user part of a URL: `scheme://user:password@host` keeps the user
//! and the host and redacts the password.

use std::ops::Range;

use super::names_another_value;

/// True for a character of a URL scheme (`https`, `git+ssh`).
fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// True for a character that ends the authority (`user:password@host:port`) of a URL:
/// the start of the path, the query or the fragment, or a character that ends a shell
/// word. A password in a URL encodes these characters.
fn ends_authority(c: char) -> bool {
    c.is_whitespace()
        || matches!(c, '/' | '?' | '#' | '\'' | '"' | '`' | ';' | '&' | '|' | '(' | ')' | '<' | '>')
}

/// The bytes of `line` that hold the password of each URL with a user and a password.
/// A password that names another value (`$TOKEN`) stays.
pub(super) fn passwords(line: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = line[from..].find("://").map(|at| from + at) {
        from = at + 3;
        let scheme = line[..at].chars().next_back().is_some_and(is_scheme_char);
        if !scheme {
            continue;
        }
        let rest = &line[from..];
        let authority = &rest[..rest.find(ends_authority).unwrap_or(rest.len())];
        // NOTE: the last `@`, because the host never holds one and a password may.
        let Some(user_end) = authority.rfind('@') else {
            continue;
        };
        let Some(colon) = authority[..user_end].find(':') else {
            continue;
        };
        let password = &authority[colon + 1..user_end];
        if !password.is_empty() && !names_another_value(password) {
            found.push(from + colon + 1..from + user_end);
        }
    }
    found
}
