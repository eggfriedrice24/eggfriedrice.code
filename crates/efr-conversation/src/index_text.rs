//! The redaction of the text that efrd's search index keeps.
//!
//! The index holds the user's prompts, the model's answers and the commands that the
//! agent ran, so a search could find a secret that one of them holds. Before a text
//! goes in, [`redact_for_index`] replaces each secret that it recognises with
//! [`MARKER`], which has no letter or digit: the tokenizer of the index makes no word
//! of it, so a search for `redacted` cannot list the places where a secret was.
//!
//! It runs two passes. The first one reads the whole text and finds what a single line
//! cannot show: a private key block (PEM), and the value of a secret in config text
//! (`password: x` in YAML, `api_key = "x"` in TOML or INI, `"token": "x"` in JSON).
//! The second one is the line redaction of the preamble (`preamble/secrets.rs`) on
//! each line, with this marker. Neither pass recognises every secret: a password in
//! prose ("my password is hunter2"), a key of lower case hex only, and a YAML value on
//! the line after its name stay. The tests list these misses.
//!
//! A change of what either pass replaces must increase [`REDACTION_VERSION`]: efrd then
//! clears the index and builds it again from the log (a full backfill).

use std::ops::Range;

use crate::preamble::secrets::{names_another_value, redact_with, replace, secret_name};

/// The version of what [`redact_for_index`] replaces. efrd writes it into the index
/// and builds the index again when it differs. Increase it with every change of the
/// patterns of `preamble/secrets.rs` or of this module.
pub const REDACTION_VERSION: u32 = 1;

/// What a redacted value reads in the index: no letter or digit, so it makes no word.
const MARKER: &str = "[***]";

/// The start of the first line of a PEM block.
const PEM_BEGIN: &str = "-----BEGIN ";

/// The start of the last line of a PEM block.
const PEM_END: &str = "-----END ";

/// The dashes that end the label of a PEM line.
const PEM_DASHES: &str = "-----";

/// The longest name of a config value that the first pass reads; a longer word is no
/// name.
const NAME_MAX: usize = 128;

/// `text` with each secret that the index redaction recognises replaced by `[***]`:
/// first each PEM block of a private key and each config value whose name looks like a
/// secret, then on each line what the preamble's line redaction finds. The lines stay
/// where they were; only the secrets change.
pub fn redact_for_index(text: &str) -> String {
    let text = replace(text, document_secrets(text), MARKER);
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&redact_with(line, MARKER));
    }
    out
}

/// The bytes of `text` that the first pass replaces: each PEM block of a private key,
/// and on each line the values of [`config_values`].
fn document_secrets(text: &str) -> Vec<Range<usize>> {
    let mut found = pem_blocks(text);
    let mut start = 0;
    for line in text.split('\n') {
        found.extend(
            config_values(line).into_iter().map(|range| start + range.start..start + range.end),
        );
        start += line.len() + 1;
    }
    found
}

/// The bytes of each private key block in `text`: from `-----BEGIN <label>-----`,
/// where the label holds `PRIVATE KEY`, to the end of the matching
/// `-----END <label>-----`, or to the end of the text when the cut of the index removed
/// that line.
fn pem_blocks(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(PEM_BEGIN).map(|at| from + at) {
        let label_at = at + PEM_BEGIN.len();
        let line_end = text[label_at..].find('\n').map_or(text.len(), |end| label_at + end);
        let Some(label_len) = text[label_at..line_end].find(PEM_DASHES) else {
            from = label_at;
            continue;
        };
        let label = &text[label_at..label_at + label_len];
        if !label.contains("PRIVATE KEY") {
            from = label_at;
            continue;
        }
        let end_line = format!("{PEM_END}{label}{PEM_DASHES}");
        let end = text[label_at..]
            .find(&end_line)
            .map_or(text.len(), |end| label_at + end + end_line.len());
        found.push(at..end);
        from = end;
    }
    found
}

/// The bytes of `line` that hold the value of a secret in config text: a JSON pair
/// `"name": value` anywhere in the line, and a line that starts with a name followed by
/// `: ` (YAML), or by `=` with a blank on either side (TOML, INI), where
/// [`secret_name`] holds for the name (its `-` and `.` read as `_`). A quoted value
/// keeps its quotes. A value that names another value (`$TOKEN`), an empty value and
/// a YAML block (`|`, `>`) stay.
fn config_values(line: &str) -> Vec<Range<usize>> {
    let mut found = json_pairs(line);
    found.extend(line_start_value(line));
    found
}

/// The values of the JSON pairs of `line` whose name looks like a secret.
fn json_pairs(line: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(open) = line[from..].find('"').map(|at| from + at) {
        let name_at = open + 1;
        let Some(name_len) = line[name_at..].find('"') else { break };
        let close = name_at + name_len;
        from = close;
        let name = &line[name_at..close];
        let after = &line[close + 1..];
        let blanks = blank_len(after);
        let Some(value_text) = after[blanks..].strip_prefix(':') else { continue };
        if !is_secret_name(name) {
            continue;
        }
        let value_at = close + 1 + blanks + 1;
        let value_at = value_at + blank_len(value_text);
        let Some(value) = value_range(line, value_at, |c| matches!(c, ',' | '}' | ']')) else {
            continue;
        };
        from = value.end.max(from);
        found.push(value);
    }
    found
}

/// The value of a line that starts with a secret name and `: ` or ` = `.
fn line_start_value(line: &str) -> Option<Range<usize>> {
    let mut at = blank_len(line);
    if let Some(rest) = line[at..].strip_prefix("- ") {
        at = line.len() - rest.len();
        at += blank_len(&line[at..]);
    }
    let rest = &line[at..];
    let (name, name_end) = match rest.chars().next()? {
        quote @ ('"' | '\'') => {
            let len = rest[1..].find(quote)?;
            (&rest[1..=len], at + len + 2)
        }
        _ => {
            let len = rest.find(|c: char| !is_config_name_char(c)).unwrap_or(rest.len());
            (&rest[..len], at + len)
        }
    };
    let before = blank_len(&line[name_end..]);
    let sign_at = name_end + before;
    let sign = line[sign_at..].chars().next()?;
    let value_at = sign_at + sign.len_utf8();
    let after = blank_len(&line[value_at..]);
    let fits = match sign {
        ':' => after > 0,
        '=' => before > 0 || after > 0,
        _ => false,
    };
    if !fits || !is_secret_name(name) {
        return None;
    }
    let value_at = value_at + after;
    let value = value_range(line, value_at, |_| false)?;
    let text = line[value.clone()].trim();
    if matches!(text, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
        return None;
    }
    Some(value)
}

/// The bytes of the value that starts at `at` in `line`: inside its quotes for a
/// quoted value (to the end of the line when the quote does not close), else up to a
/// character for which `ends` holds, or to the end of the line, without the blanks at
/// its end. `None` for an empty value and one that names another value.
fn value_range(line: &str, at: usize, ends: impl Fn(char) -> bool) -> Option<Range<usize>> {
    let rest = &line[at..];
    let range = match rest.chars().next()? {
        quote @ ('"' | '\'') => {
            let body = &rest[1..];
            let len = closing_quote(body, quote).unwrap_or(body.len());
            at + 1..at + 1 + len
        }
        _ => {
            let len = rest.find(|c: char| ends(c)).unwrap_or(rest.len());
            at..at + rest[..len].trim_end().len()
        }
    };
    let value = &line[range.clone()];
    (!value.trim().is_empty() && !names_another_value(value)).then_some(range)
}

/// Where the `quote` that closes `body` stands; a backslash keeps the character after
/// it inside double quotes.
fn closing_quote(body: &str, quote: char) -> Option<usize> {
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        if c == '\\' && quote == '"' {
            chars.next();
        } else if c == quote {
            return Some(at);
        }
    }
    None
}

/// True for a name of config text that looks like a secret.
fn is_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name.chars().all(is_config_name_char)
        && secret_name(&name.replace(['-', '.'], "_"))
}

/// True for a character of a name in config text.
fn is_config_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// The length of the blanks (spaces and tabs) at the start of `text`.
fn blank_len(text: &str) -> usize {
    text.len() - text.trim_start_matches([' ', '\t']).len()
}

#[cfg(test)]
mod tests;
