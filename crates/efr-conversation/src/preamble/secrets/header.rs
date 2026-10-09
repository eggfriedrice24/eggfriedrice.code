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

/// The authorization schemes, compared without case. A value that is one of them alone
/// holds no secret, as in the pattern of `grep -r "Authorization: Bearer" .`.
const SCHEMES: &[&str] = &[
    "basic",
    "bearer",
    "digest",
    "token",
    "negotiate",
    "ntlm",
    "hoba",
    "mutual",
    "vapid",
    "oauth",
    "bot",
    "aws4-hmac-sha256",
    "scram-sha-1",
    "scram-sha-256",
];

/// The options that take a header as their next word.
const HEADER_OPTIONS: &[&str] = &["-H", "--header"];

/// The HTTP clients that take a header as a plain argument (`Name:value`).
const HEADER_CLIENTS: &[&str] = &["http", "https", "xh", "xhs"];

/// The bytes of the line that hold the value of each secret header in `commands`.
///
/// A header is a word, or the part of a word after `-H` or after its first `=`, that
/// starts with a header name and a colon, where a header stands: after `-H` or
/// `--header` (also in the same word), after a `=`, at the start of a command (a line
/// of a here-document), among the arguments of an HTTP client that takes plain
/// headers (`http`, `xh`), or after `echo` or `printf` for the headers that always
/// carry a secret (`Authorization`, `Cookie`). Elsewhere such a word is an argument,
/// such as the pattern of `grep -r "password: true" .`. Its value is the rest of the
/// word (a header in quotes). When the colon ends the word, the value is the words
/// after it in the same command, but only at the start of a command or after `echo`
/// or `printf`. A value that names another value (`$TOKEN`), and a value that is only
/// an authorization scheme (`Bearer`), stays.
pub(super) fn values(commands: &[Vec<Word>]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    for command in commands {
        let program = command.first().map_or("", |word| program_name(&word.text));
        let echoes = ECHOES.contains(&program);
        for (index, word) in command.iter().enumerate() {
            let Some((name, value_at, start)) = header(&word.text) else {
                continue;
            };
            let always = SECRET_HEADERS.iter().any(|header| name.eq_ignore_ascii_case(header));
            let after_option = index
                .checked_sub(1)
                .and_then(|before| command.get(before))
                .is_some_and(|before| takes_header(&before.text));
            let stands = start > 0
                || index == 0
                || after_option
                || HEADER_CLIENTS.contains(&program)
                || (echoes && always);
            if !stands {
                continue;
            }
            let scheme = SCHEME_HEADERS.iter().any(|header| name.eq_ignore_ascii_case(header));
            if value_at < word.text.len() {
                let value = &word.text[value_at..];
                let from = value_at + if scheme { scheme_len(value) } else { 0 };
                let to = value_at + value.trim_end().len();
                let secret = &word.text[from..to.max(from)];
                if from < to && !names_another_value(secret) && !(scheme && is_known_scheme(secret))
                {
                    found.extend(word.line_range_to(from, to));
                }
            } else if index == 0 || (index == 1 && echoes) {
                let rest = &command[index + 1..];
                let skip = usize::from(scheme && rest.len() > 1 && is_scheme(&rest[0].text));
                let (Some(first), Some(last)) = (rest.get(skip), rest.last()) else {
                    continue;
                };
                let alone = rest.len() == 1 && scheme && is_known_scheme(&first.text);
                if alone || names_another_value(&first.text) {
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

/// The name of the header in `text`, where its value starts (after the colon and the
/// blanks after it) and where the header starts. The header starts the text, follows a
/// leading `-H`, or follows the first `=` (`--header=`, `http.extraHeader=`).
fn header(text: &str) -> Option<(&str, usize, usize)> {
    let starts = [Some(0), text.starts_with("-H").then_some(2), text.find('=').map(|at| at + 1)];
    starts
        .into_iter()
        .flatten()
        .find_map(|start| header_at(text, start).map(|(name, value_at)| (name, value_at, start)))
}

/// The name of the program that a command's first word runs, without its directory.
fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// True for a word after which a header follows: `-H`, `--header`, or short options
/// in one word that end with `H` (`-sH`).
fn takes_header(word: &str) -> bool {
    if HEADER_OPTIONS.contains(&word) {
        return true;
    }
    word.strip_prefix('-').is_some_and(|flags| {
        !flags.starts_with('-')
            && flags.ends_with('H')
            && flags.chars().all(|c| c.is_ascii_alphabetic())
    })
}

/// True for a value that is only an authorization scheme, such as `Bearer`.
fn is_known_scheme(value: &str) -> bool {
    SCHEMES.iter().any(|scheme| value.eq_ignore_ascii_case(scheme))
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
