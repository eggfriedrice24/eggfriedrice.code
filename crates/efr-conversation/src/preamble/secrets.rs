//! The user's last command as the preamble shows it, with the values that look like
//! secrets redacted.
//!
//! The preamble is saved with the turn's messages and every later request sends it
//! again, so a secret in the last command, such as `export TOKEN=...`, would reach the
//! disk and every request after it. [`redact`] runs when the preamble is rendered, so
//! the bytes that the store keeps are the bytes that the model read. It is a pattern
//! list, not a shell parser: it redacts too much rather than too little, but it does
//! not touch words that only look like the patterns (`docker run -p 8080:80`,
//! `http://host:8080/`, `grep -r "password:" .`).

mod header;
mod options;
mod url;
mod words;

use std::ops::Range;

use efr_sandbox::secret_like;

/// What a redacted value reads.
pub(crate) const REDACTED: &str = "[redacted]";

/// The starts of API keys and tokens that are secret wherever they stand, longest
/// first.
const KEY_PREFIXES: &[&str] =
    &["sk-ant-", "sk-proj-", "github_pat_", "sk-", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// The fewest characters after a key prefix that make a key; a shorter word, such as
/// `sk-learn`, is a name.
const KEY_MIN_CHARS: usize = 16;

/// `line` with each secret replaced by [`REDACTED`]:
///
/// - the value of an assignment `NAME=value` whose name looks like a secret
///   ([`secret_like`], with the dashes of an option such as `--api-key=` read as
///   underscores), unquoted or in quotes;
/// - each word that starts with a key prefix ([`KEY_PREFIXES`]) followed by at least
///   [`KEY_MIN_CHARS`] key characters;
/// - the password of a URL, `scheme://user:password@host` ([`url::passwords`]);
/// - the value of a header that carries a secret, such as `Authorization: Bearer x`,
///   without its scheme, where a header stands ([`header::values`]);
/// - the value of an option that takes a password, for the programs known to take one
///   there, such as `mysql -psecret` ([`options::values`]).
///
/// A value that starts with `$` or a backtick names another value and stays. When two
/// secrets overlap or touch, one marker replaces both.
pub(crate) fn redact(line: &str) -> String {
    let commands = words::commands(line);
    let mut found = assignments(line);
    found.extend(keys(line));
    found.extend(url::passwords(line));
    found.extend(header::values(&commands));
    found.extend(options::values(&commands));
    replace(line, found)
}

/// `line` with each of the `found` byte ranges replaced by [`REDACTED`]; ranges that
/// overlap or touch get one marker.
fn replace(line: &str, mut found: Vec<Range<usize>>) -> String {
    found.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in found.into_iter().filter(|range| !range.is_empty()) {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    for range in merged {
        out.push_str(&line[at..range.start]);
        out.push_str(REDACTED);
        at = range.end;
    }
    out.push_str(&line[at..]);
    out
}

/// True for a value that names another value: `$NAME`, `$(...)`, `${...}` or a
/// backtick. `$'...'` is a quoted value, not a name.
fn names_another_value(value: &str) -> bool {
    value.starts_with('`') || (value.starts_with('$') && !value.starts_with("$'"))
}

/// True for a character of a name before `=`.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// True for a character that ends an unquoted value.
fn ends_value(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\'' | '"' | '`' | ';' | '&' | '|' | '(' | ')' | '<' | '>')
}

/// The bytes of `line` that hold the value of each assignment to a secret-like name.
/// The value of any other assignment is read again, because it can hold one, such as
/// `--from-literal=password=x` or `--env=API_TOKEN=x`.
fn assignments(line: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = line[from..].find('=').map(|at| from + at) {
        let before = &line[from..at];
        let name_len: usize =
            before.chars().rev().take_while(|c| is_name_char(*c)).map(char::len_utf8).sum();
        let name = before[before.len() - name_len..].trim_start_matches('-').replace('-', "_");
        let start = at + 1;
        let value = value_len(&line[start..]);
        from = start;
        if value > 0 && !name.is_empty() && secret_like(&name) {
            found.push(start..start + value);
            from = start + value;
        }
    }
    found
}

/// The length in bytes of the value at the start of `text`: a quoted value with its
/// quotes (also `$'...'`), up to the closing quote or the end, or an unquoted value up
/// to the first character that ends it. 0 for no value, and for a value that names
/// another one (`$NAME`, `$(...)`, `${...}` or a backtick).
fn value_len(text: &str) -> usize {
    if let Some(quoted) = text.strip_prefix('$').filter(|rest| rest.starts_with('\'')) {
        return 1 + quoted_len(quoted, '\'');
    }
    match text.chars().next() {
        None | Some('`' | '$') => 0,
        Some(quote @ ('\'' | '"')) => quoted_len(text, quote),
        Some(_) => text.find(ends_value).unwrap_or(text.len()),
    }
}

/// The length in bytes of the value in `quote`s at the start of `text`, with its
/// quotes; all of `text` when the quote does not close.
fn quoted_len(text: &str, quote: char) -> usize {
    text[1..].find(quote).map_or(text.len(), |at| at + 2)
}

/// The bytes of `line` that hold each word in the form of a known key.
fn keys(line: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < line.len() {
        let rest = &line[at..];
        let starts_word = line[..at].chars().next_back().is_none_or(|c| !is_name_char(c));
        let key = KEY_PREFIXES.iter().find_map(|prefix| {
            let body = rest.strip_prefix(prefix)?;
            let chars = body.find(|c: char| !is_name_char(c)).unwrap_or(body.len());
            (chars >= KEY_MIN_CHARS).then_some(prefix.len() + chars)
        });
        match key {
            Some(len) if starts_word => {
                found.push(at..at + len);
                at += len;
            }
            _ => at += rest.chars().next().map_or(1, char::len_utf8),
        }
    }
    found
}

#[cfg(test)]
mod tests;
