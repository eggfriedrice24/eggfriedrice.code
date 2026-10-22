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
//!
//! The limit: a key with no known prefix is found only by its form, a long word of
//! upper and lower case letters and digits ([`random_words`]). A key of only lower case
//! hex or base32 characters stays visible, and so do git ids and sha256 sums, which
//! have the same form.

mod header;
mod options;
mod url;
mod words;

use std::ops::Range;

use efr_sandbox::secret_like;

/// What a redacted value reads.
pub(crate) const REDACTED: &str = "[redacted]";

/// The starts of API keys and tokens that are secret wherever they stand, longest
/// first, each with the characters of the key after it and the fewest of them that
/// make a key; a shorter word, such as `sk-learn`, is a name. The keys of npm and
/// Hugging Face have only letters and digits after the prefix, so a name such as
/// `npm_config_userconfig` stays.
const KEY_PREFIXES: &[KeyPrefix] = &[
    KeyPrefix::name("sk-ant-"),
    KeyPrefix::name("sk-proj-"),
    KeyPrefix::name("github_pat_"),
    KeyPrefix::name("glpat-"),
    KeyPrefix::name("pypi-"),
    KeyPrefix::name("xoxb-"),
    KeyPrefix::name("xoxp-"),
    KeyPrefix::name("sk-"),
    KeyPrefix::name("ghp_"),
    KeyPrefix::name("gho_"),
    KeyPrefix::name("ghu_"),
    KeyPrefix::name("ghs_"),
    KeyPrefix::name("ghr_"),
    KeyPrefix { prefix: "AKIA", body: is_upper_or_digit, min: 16 },
    KeyPrefix { prefix: "AIza", body: is_name_char, min: 30 },
    KeyPrefix { prefix: "npm_", body: is_letter_or_digit, min: 30 },
    KeyPrefix { prefix: "hf_", body: is_letter_or_digit, min: 30 },
];

/// The start of a key and the form of the rest.
struct KeyPrefix {
    /// The prefix.
    prefix: &'static str,
    /// True for a character of the key after the prefix.
    body: fn(char) -> bool,
    /// The fewest such characters that make a key.
    min: usize,
}

impl KeyPrefix {
    /// A prefix followed by at least 16 characters of a name (letters, digits, `_`
    /// and `-`).
    const fn name(prefix: &'static str) -> Self {
        KeyPrefix { prefix, body: is_name_char, min: 16 }
    }
}

/// The fewest characters of a word that [`random_words`] reads as a key.
const RANDOM_MIN_CHARS: usize = 32;

/// The lengths of a git id (sha1) and of a sha256 sum in hex.
const HEX_ID_LENGTHS: [usize; 2] = [40, 64];

/// The last words of a name that make it a number, such as `TOKEN_COUNT` or
/// `PASSWORD_LENGTH`, and not a secret.
const COUNT_WORDS: &[&str] = &["COUNT", "COUNTS", "LIMIT", "LEN", "LENGTH", "SIZE", "MAX", "MIN"];

/// The shortest name that [`secret_like`] can match, such as `_KEY`.
const SHORTEST_SECRET_NAME: usize = 4;

/// Words of a name that [`secret_like`] matches but that name a count or a model part,
/// such as `MAX_TOKENS` or `TOKENIZER`, and not a secret.
const COUNTED_WORDS: &[&str] = &["TOKENS", "TOKENIZER", "TOKENIZERS"];

/// `line` with each secret replaced by [`REDACTED`]:
///
/// - the value of an assignment `NAME=value` whose name looks like a secret
///   ([`secret_name`], with the dashes of an option such as `--api-key=` read as
///   underscores), unquoted or in quotes;
/// - each word that starts with a key prefix ([`KEY_PREFIXES`]) followed by enough
///   key characters;
/// - each word that looks like a random key ([`random_words`]), and each JSON web
///   token ([`web_tokens`]);
/// - the password of a URL, `scheme://user:password@host` ([`url::passwords`]);
/// - the value of a header that carries a secret, such as `Authorization: Bearer x`,
///   without its scheme, where a header stands ([`header::values`]);
/// - the value of an option that takes a password, for the programs known to take one
///   there, such as `mysql -psecret` ([`options::values`]).
///
/// A value that starts with `$` or a backtick names another value and stays. When two
/// secrets overlap or touch, one marker replaces both.
pub(crate) fn redact(line: &str) -> String {
    redact_with(line, REDACTED)
}

/// [`redact`] with `marker` in place of [`REDACTED`]: the same secrets, another mark.
/// The search index writes a marker with no letter or digit, so it makes no word.
pub(crate) fn redact_with(line: &str, marker: &str) -> String {
    let commands = words::commands(line);
    let mut found = assignments(line);
    found.extend(keys(line));
    found.extend(random_words(line));
    found.extend(web_tokens(line));
    found.extend(url::passwords(line));
    found.extend(header::values(&commands));
    found.extend(options::values(&commands));
    replace(line, found, marker)
}

/// `line` with each of the `found` byte ranges replaced by `marker`; ranges that
/// overlap or touch get one marker.
pub(crate) fn replace(line: &str, mut found: Vec<Range<usize>>, marker: &str) -> String {
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
        out.push_str(marker);
        at = range.end;
    }
    out.push_str(&line[at..]);
    out
}

/// True when `name` (words joined by `_`) looks like a secret: [`secret_like`] matches
/// it, and it is not a number (its last word is one of [`COUNT_WORDS`]) and not a
/// secret only through one of [`COUNTED_WORDS`]. So `GITHUB_TOKEN` is one, and
/// `MAX_TOKENS`, `TOKENIZER` and `TOKEN_COUNT` are not.
pub(crate) fn secret_name(name: &str) -> bool {
    // NOTE: every pattern of `secret_like` needs 4 characters or more (`*_KEY`,
    // `AWS_*`), so a shorter name is none without the allocations of a match: a text
    // with thousands of `a=` stays fast.
    if name.len() < SHORTEST_SECRET_NAME || !secret_like(name) {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    let words: Vec<&str> = upper.split('_').filter(|word| !word.is_empty()).collect();
    if words.last().is_some_and(|word| COUNT_WORDS.contains(word)) {
        return false;
    }
    let rest: Vec<&str> = words.into_iter().filter(|word| !COUNTED_WORDS.contains(word)).collect();
    secret_like(&rest.join("_"))
}

/// True for a value that names another value: `$NAME`, `$(...)`, `${...}` or a
/// backtick. `$'...'` is a quoted value, not a name.
pub(crate) fn names_another_value(value: &str) -> bool {
    value.starts_with('`') || (value.starts_with('$') && !value.starts_with("$'"))
}

/// True for a character of a name before `=`.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// True for an ASCII letter or digit.
fn is_letter_or_digit(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// True for an ASCII upper case letter or digit.
fn is_upper_or_digit(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

/// True for a character of a word in base64 or in its url-safe form, without the `=`
/// of the padding.
fn is_base64_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '_' | '-')
}

/// True for a character of url-safe base64.
fn is_url_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
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
        from = start;
        // NOTE: the name first: a value is read only for a secret name, so a line of
        // many `=` with long values (`a="a="a="...`) costs one pass, not one per `=`.
        if name.is_empty() || !secret_name(&name) {
            continue;
        }
        let value = value_len(&line[start..]);
        if value > 0 {
            found.push(start..start + value);
            from = start + value;
        }
    }
    found
}

/// The length in bytes of the value at the start of `text`: a quoted value with its
/// quotes (also `$'...'`), up to the closing quote or the end, or an unquoted value up
/// to the first character that ends it, where a backslash keeps the character after
/// it (`abc\ def`). 0 for no value, and for a value that names another one (`$NAME`,
/// `$(...)`, `${...}` or a backtick).
fn value_len(text: &str) -> usize {
    if let Some(quoted) = text.strip_prefix('$').filter(|rest| rest.starts_with('\'')) {
        return 1 + quoted_len(quoted, '\'');
    }
    match text.chars().next() {
        None | Some('`' | '$') => 0,
        Some(quote @ ('\'' | '"')) => quoted_len(text, quote),
        Some(_) => unquoted_len(text),
    }
}

/// The length in bytes of the unquoted value at the start of `text`: up to the first
/// character that ends it, a backslash with the character after it included.
fn unquoted_len(text: &str) -> usize {
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        if c == '\\' {
            if chars.next().is_none() {
                return text.len();
            }
        } else if ends_value(c) {
            return at;
        }
    }
    text.len()
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
        // NOTE: the start of a word first: the body of a key is read only there, so a
        // long run of key characters (`xsk-sk-sk-...`) costs one pass, not one per
        // character.
        let key = starts_word
            .then(|| {
                KEY_PREFIXES.iter().find_map(|key| {
                    let body = rest.strip_prefix(key.prefix)?;
                    let chars = body.find(|c: char| !(key.body)(c)).unwrap_or(body.len());
                    (chars >= key.min).then_some(key.prefix.len() + chars)
                })
            })
            .flatten();
        match key {
            Some(len) => {
                found.push(at..at + len);
                at += len;
            }
            _ => at += rest.chars().next().map_or(1, char::len_utf8),
        }
    }
    found
}

/// The bytes of `line` that hold each word that looks like a random key: a word of
/// base64 or url-safe base64 characters (with the `=` of its padding) of at least
/// [`RANDOM_MIN_CHARS`] characters, with upper and lower case letters and digits. Hex
/// of the length of a git id or a sha256 sum stays, and so does a path: a word with a
/// part between two `/` of three or more lower case letters, such as `a/src/B`, or with
/// two or more `/` and such a first part, such as `src/A/B` or (once its first `/` is
/// left out, as every `/` at the start of a word) `/home/u/A`. The last part never
/// counts, and neither does a first part before the only `/`: a random key of base64
/// has a `/` and a short run of lower case letters often enough. A long
/// kebab-case or snake_case name has no upper case letter, so it stays too.
fn random_words(line: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in line.char_indices().chain(std::iter::once((line.len(), ' '))) {
        if is_base64_char(c) {
            start.get_or_insert(at);
            continue;
        }
        let Some(from) = start.take() else { continue };
        // NOTE: the `/` at the start of a word, such as the `//` of a URL, is no part
        // of a key; the rest of the word is judged alone.
        let from = from + line[from..at].bytes().take_while(|byte| *byte == b'/').count();
        if looks_random(&line[from..at]) {
            let padding = line[at..].bytes().take_while(|byte| *byte == b'=').take(2).count();
            found.push(from..at + padding);
        }
    }
    found
}

/// True for a `word` of [`random_words`] that looks like a random key.
fn looks_random(word: &str) -> bool {
    if word.len() < RANDOM_MIN_CHARS {
        return false;
    }
    let upper = word.chars().any(|c| c.is_ascii_uppercase());
    let lower = word.chars().any(|c| c.is_ascii_lowercase());
    let digit = word.chars().any(|c| c.is_ascii_digit());
    if !(upper && lower && digit) {
        return false;
    }
    if HEX_ID_LENGTHS.contains(&word.len()) && word.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    !looks_like_a_path(word)
}

/// True for a `word` of [`random_words`] that looks like a path (see there).
fn looks_like_a_path(word: &str) -> bool {
    let lower_part = |part: &&str| part.len() >= 3 && part.chars().all(|c| c.is_ascii_lowercase());
    let parts: Vec<&str> = word.split('/').collect();
    let Some((first, rest)) = parts.split_first() else { return false };
    let inner = rest.split_last().map_or(&[][..], |(_, inner)| inner);
    inner.iter().any(lower_part) || (!inner.is_empty() && lower_part(first))
}

/// The bytes of `line` that hold each JSON web token: a word that starts with `eyJ`
/// (the base64 of `{"`) and has three parts of url-safe base64 joined by `.`; the
/// last part, the signature, can be empty.
fn web_tokens(line: &str) -> Vec<Range<usize>> {
    let run = |text: &str| text.find(|c: char| !is_url_safe_char(c)).unwrap_or(text.len());
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = line[from..].find("eyJ").map(|at| from + at) {
        from = at + 1;
        if line[..at].chars().next_back().is_some_and(is_url_safe_char) {
            continue;
        }
        let header = run(&line[at..]);
        let Some(payload) = line[at + header..].strip_prefix('.') else { continue };
        let payload_len = run(payload);
        let Some(signature) = payload[payload_len..].strip_prefix('.') else { continue };
        if payload_len == 0 {
            continue;
        }
        let end = at + header + payload_len + run(signature) + 2;
        found.push(at..end);
        from = end;
    }
    found
}

#[cfg(test)]
mod tests;
