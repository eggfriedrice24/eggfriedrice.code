//! The value of a header that carries a secret: `Authorization: Bearer x`,
//! `-H 'X-Api-Key: x'`, `Cookie: session=x`. The authorization scheme stays, so the
//! line still says how the request signs in.

use std::ops::Range;

use efr_sandbox::secret_like;

use super::names_another_value;
use super::words::Word;

/// Headers whose value is a secret, compared without case; a name that
/// [`secret_like`] matches, with dashes read as underscores, is one too.
const SECRET_HEADERS: &[&str] = &["authorization", "proxy-authorization", "cookie"];

/// The commands that print their words, so a header may stand among them unquoted.
const ECHOES: &[&str] = &["echo", "printf"];

/// Headers whose value starts with a scheme that stays (`Bearer`, `Basic`, `token`).
const SCHEME_HEADERS: &[&str] = &["authorization", "proxy-authorization"];

/// The bytes of the line that hold the value of each secret header in `commands`.
///
/// A header is a word, or the part of a word after `-H` or after its first `=`, that
/// starts with a header name and a colon. Its value is the rest of the word (a
/// header in quotes). When the colon ends the word, the value is the words after it in
/// the same command, but only for a header that starts the command (a line of a
/// here-document) or follows `echo` or `printf`: elsewhere such a word is an argument,
/// such as the pattern of `grep -r "password:" .`. A value that names another value
/// (`$TOKEN`) stays.
pub(super) fn values(commands: &[Vec<Word>]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    for command in commands {
        for (index, word) in command.iter().enumerate() {
            let Some((name, value_at)) = header(&word.text) else {
                continue;
            };
            let scheme = SCHEME_HEADERS.iter().any(|header| name.eq_ignore_ascii_case(header));
            if value_at < word.text.len() {
                let value = &word.text[value_at..];
                let from = value_at + if scheme { scheme_len(value) } else { 0 };
                let to = value_at + value.trim_end().len();
                if from < to && !names_another_value(&word.text[from..]) {
                    found.extend(word.line_range_to(from, to));
                }
            } else if index == 0 || (index == 1 && ECHOES.contains(&command[0].text.as_str())) {
                let rest = &command[index + 1..];
                let skip = usize::from(scheme && rest.len() > 1 && is_scheme(&rest[0].text));
                let (Some(first), Some(last)) = (rest.get(skip), rest.last()) else {
                    continue;
                };
                if names_another_value(&first.text) {
                    continue;
                }
                if let (Some(start), Some(end)) = (first.line_range(0), last.line_range(0)) {
                    found.push(start.start..end.end);
                }
            }
        }
    }
    found
}

/// The name of the header in `text` and where its value starts (after the colon and
/// the blanks after it). The header starts the text, follows a leading `-H`, or follows
/// the first `=` (`--header=`, `http.extraHeader=`).
fn header(text: &str) -> Option<(&str, usize)> {
    let starts = [Some(0), text.starts_with("-H").then_some(2), text.find('=').map(|at| at + 1)];
    starts.into_iter().flatten().find_map(|start| header_at(text, start))
}

/// The header that starts at byte `start` of `text`, as [`header`] returns it.
fn header_at(text: &str, start: usize) -> Option<(&str, usize)> {
    let rest = &text[start..];
    let name_len = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))?;
    let name = &rest[..name_len];
    let after = rest[name_len..].strip_prefix(':')?;
    // NOTE: `a::b` is a path and `x://` a URL, not a header.
    if name.is_empty() || after.starts_with(':') || after.starts_with("//") {
        return None;
    }
    let secret = SECRET_HEADERS.iter().any(|header| name.eq_ignore_ascii_case(header))
        || secret_like(&name.replace('-', "_"));
    if !secret {
        return None;
    }
    let blanks = after.len() - after.trim_start_matches([' ', '\t']).len();
    Some((name, text.len() - after.len() + blanks))
}

/// True for a word that names an authorization scheme: letters, digits and dashes,
/// starting with a letter (`Bearer`, `token`, `AWS4-HMAC-SHA256`).
fn is_scheme(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_alphabetic())
        && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The length of the scheme and the blanks after it at the start of `value`; 0 when
/// the value is one word, which is then the secret itself.
fn scheme_len(value: &str) -> usize {
    let Some(end) = value.find([' ', '\t']) else {
        return 0;
    };
    let rest = value[end..].trim_start_matches([' ', '\t']);
    if is_scheme(&value[..end]) && !rest.trim_end().is_empty() {
        value.len() - rest.len()
    } else {
        0
    }
}
