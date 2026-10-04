//! Placeholders for the parts of a transcript that change from run to run.
//!
//! A transcript recorded in one temporary directory must replay in another, on
//! another machine, at another time. [`Redactor::redact`] replaces the temporary
//! working directory, the scratch path, the host name and RFC 3339 timestamps with
//! fixed placeholders before outbound traffic is compared; [`Redactor::restore`] puts
//! the real paths back into inbound records, so a model that names `<CWD>` in a
//! fixture sends the real directory to the code under test.

use std::path::Path;

use serde_json::Value;

/// Replaces the values of one run with placeholders, and back.
///
/// Values match only as whole names: `/t/proj` is not redacted inside `/t/project2`,
/// and the host name `box` is not redacted inside `inbox` or `box-2`. When two values
/// overlap, the longer one wins, so a working directory inside the temporary root
/// becomes `<CWD>`, not `<TMP>/home/project`.
///
/// ```ignore
/// let redactor = Redactor::new().cwd("/tmp/efr-test-x/home/p").hostname("box");
/// assert_eq!(redactor.redact("box: ls /tmp/efr-test-x/home/p"), "<HOSTNAME>: ls <CWD>");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redactor {
    /// In the order they were added, which decides the value a shared placeholder is
    /// restored to.
    pairs: Vec<Pair>,
    timestamps: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pair {
    actual: String,
    placeholder: String,
}

impl Redactor {
    /// The placeholder of the working directory of a test.
    pub const CWD: &'static str = "<CWD>";
    /// The placeholder of a conversation's scratch directory.
    pub const SCRATCH: &'static str = "<SCRATCH>";
    /// The placeholder of the host name.
    pub const HOSTNAME: &'static str = "<HOSTNAME>";
    /// The placeholder of an RFC 3339 timestamp with an offset.
    pub const TIMESTAMP: &'static str = "<TIMESTAMP>";
    /// The placeholder of a test's temporary root, for any temporary path that no
    /// more specific value covers.
    pub const TEMP_ROOT: &'static str = "<TMP>";

    /// A redactor that replaces only timestamps.
    pub fn new() -> Self {
        Redactor { pairs: Vec::new(), timestamps: true }
    }

    /// Also replaces the working directory `path` with [`Redactor::CWD`].
    #[must_use]
    pub fn cwd(self, path: impl AsRef<Path>) -> Self {
        self.path(path.as_ref(), Self::CWD)
    }

    /// Also replaces the scratch directory `path` with [`Redactor::SCRATCH`].
    #[must_use]
    pub fn scratch(self, path: impl AsRef<Path>) -> Self {
        self.path(path.as_ref(), Self::SCRATCH)
    }

    /// Also replaces the temporary root `path` with [`Redactor::TEMP_ROOT`].
    #[must_use]
    pub fn temp_root(self, path: impl AsRef<Path>) -> Self {
        self.path(path.as_ref(), Self::TEMP_ROOT)
    }

    /// Also replaces the host name `name` with [`Redactor::HOSTNAME`].
    #[must_use]
    pub fn hostname(self, name: impl Into<String>) -> Self {
        self.replace(name, Self::HOSTNAME)
    }

    /// Also replaces `actual` with `placeholder`. An empty `actual` is ignored.
    #[must_use]
    pub fn replace(mut self, actual: impl Into<String>, placeholder: impl Into<String>) -> Self {
        let actual = actual.into();
        if !actual.is_empty() {
            self.pairs.push(Pair { actual, placeholder: placeholder.into() });
        }
        self
    }

    /// Leaves timestamps as they are.
    #[must_use]
    pub fn keep_timestamps(mut self) -> Self {
        self.timestamps = false;
        self
    }

    /// `text` with every registered value and, unless kept, every timestamp replaced by
    /// its placeholder.
    pub fn redact(&self, text: &str) -> String {
        let mut by_length: Vec<&Pair> = self.pairs.iter().collect();
        by_length.sort_by_key(|pair| std::cmp::Reverse(pair.actual.len()));
        rewrite(text, |at| {
            let rest = &text[at..];
            for pair in &by_length {
                let end = at + pair.actual.len();
                if rest.starts_with(&pair.actual) && whole_name(text, at, end) {
                    return Some((pair.actual.len(), pair.placeholder.as_str()));
                }
            }
            if self.timestamps && !preceded_by_alphanumeric(text, at) {
                return timestamp_len(rest).map(|len| (len, Self::TIMESTAMP));
            }
            None
        })
    }

    /// `text` with every placeholder of a registered value replaced by that value. A
    /// placeholder shared by several values gets the one added first; timestamps stay
    /// redacted, because the original is gone.
    pub fn restore(&self, text: &str) -> String {
        let mut by_length: Vec<&Pair> = self.pairs.iter().collect();
        by_length.sort_by_key(|pair| std::cmp::Reverse(pair.placeholder.len()));
        rewrite(text, |at| {
            let rest = &text[at..];
            by_length
                .iter()
                .find(|pair| !pair.placeholder.is_empty() && rest.starts_with(&pair.placeholder))
                .map(|pair| (pair.placeholder.len(), pair.actual.as_str()))
        })
    }

    /// [`redact`](Self::redact) applied to every string in `value`, object keys
    /// included.
    pub fn redact_json(&self, value: &Value) -> Value {
        map_strings(value, &|text| self.redact(text))
    }

    /// [`restore`](Self::restore) applied to every string in `value`, object keys
    /// included.
    pub fn restore_json(&self, value: &Value) -> Value {
        map_strings(value, &|text| self.restore(text))
    }

    /// Registers a path without its trailing `/`. The root and the empty path are
    /// ignored, because replacing every `/` would make any text unreadable.
    fn path(self, path: &Path, placeholder: &str) -> Self {
        let text = path.to_string_lossy();
        let trimmed = text.trim_end_matches('/');
        if trimmed.is_empty() { self } else { self.replace(trimmed, placeholder) }
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Redactor::new()
    }
}

/// Copies `text`, asking `replacement` at each character whether a span starting there
/// is replaced, and with what.
fn rewrite<'a>(text: &str, replacement: impl Fn(usize) -> Option<(usize, &'a str)>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(ch) = text[at..].chars().next() {
        match replacement(at) {
            Some((len, with)) if len > 0 => {
                out.push_str(with);
                at += len;
            }
            _ => {
                out.push(ch);
                at += ch.len_utf8();
            }
        }
    }
    out
}

fn map_strings(value: &Value, f: &impl Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(f(text)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| map_strings(item, f)).collect())
        }
        Value::Object(members) => Value::Object(
            members.iter().map(|(name, item)| (f(name), map_strings(item, f))).collect(),
        ),
        other => other.clone(),
    }
}

/// True when `text[start..end]` is not part of a longer name: an alphanumeric first
/// character must not follow a name character, and an alphanumeric last character must
/// not be followed by one. A `.` after the span is allowed, so a path at the end of a
/// sentence and a host name followed by its domain both match.
fn whole_name(text: &str, start: usize, end: usize) -> bool {
    let span = &text[start..end];
    let starts_alnum = span.chars().next().is_some_and(char::is_alphanumeric);
    let ends_alnum = span.chars().next_back().is_some_and(char::is_alphanumeric);
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    let before_ok =
        !starts_alnum || !before.is_some_and(|c| c.is_alphanumeric() || "-_.".contains(c));
    let after_ok = !ends_alnum || !after.is_some_and(|c| c.is_alphanumeric() || "-_".contains(c));
    before_ok && after_ok
}

fn preceded_by_alphanumeric(text: &str, at: usize) -> bool {
    text[..at].chars().next_back().is_some_and(char::is_alphanumeric)
}

/// The length of the RFC 3339 timestamp at the start of `text`, if one is there:
/// `YYYY-MM-DDTHH:MM:SS`, an optional fraction of 1 to 9 digits, then `Z` or an offset
/// `+HH:MM` or `-HH:MM`. `t`, `z` and a space separator are accepted as RFC 3339
/// allows. The fields must be in range, and no letter or digit may follow.
fn timestamp_len(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let digits = |from: usize, count: usize| {
        b.get(from..from + count).is_some_and(|run| run.iter().all(u8::is_ascii_digit))
    };
    let is = |at: usize, expected: u8| b.get(at) == Some(&expected);
    let two = |at: usize| (b[at] - b'0') * 10 + (b[at + 1] - b'0');
    let shape = digits(0, 4)
        && is(4, b'-')
        && digits(5, 2)
        && is(7, b'-')
        && digits(8, 2)
        && matches!(b.get(10), Some(b'T' | b't' | b' '))
        && digits(11, 2)
        && is(13, b':')
        && digits(14, 2)
        && is(16, b':')
        && digits(17, 2);
    if !shape {
        return None;
    }
    let in_range = (1..=12).contains(&two(5))
        && (1..=31).contains(&two(8))
        && two(11) <= 23
        && two(14) <= 59
        && two(17) <= 60;
    if !in_range {
        return None;
    }
    let mut end = 19;
    if is(end, b'.') {
        let fraction = b[end + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if !(1..=9).contains(&fraction) {
            return None;
        }
        end += 1 + fraction;
    }
    match b.get(end) {
        Some(b'Z' | b'z') => end += 1,
        Some(b'+' | b'-')
            if digits(end + 1, 2)
                && is(end + 3, b':')
                && digits(end + 4, 2)
                && two(end + 1) <= 23
                && two(end + 4) <= 59 =>
        {
            end += 6;
        }
        _ => return None,
    }
    if b.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    Some(end)
}

#[cfg(test)]
mod tests;
