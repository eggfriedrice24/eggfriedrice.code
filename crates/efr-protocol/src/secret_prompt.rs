//! The guess whether a line of a program's output asks for a secret.
//!
//! The daemon sets `looks_secret` of `tool_call_input_changed` with it, and a client
//! uses it on a running call's output before the daemon reports a wait, so keys typed
//! for a password prompt never go anywhere that shows or sends them. It is a guess from
//! text that a program printed: it only changes what a client shows and where it keeps
//! keys, never where an answer goes.

/// What a prompt that asks for a secret says, in lower case. `pin` counts only as a
/// word of its own, so `ping` or `spinning` do not.
const SECRET_WORDS: &[&str] =
    &["password", "passphrase", "passcode", "verification code", "one-time code", "one time code"];

/// True when `line` reads like a prompt for a secret: it names a password, a
/// passphrase, a passcode, a PIN, a verification code or a one-time code, in any case.
pub fn looks_secret(line: &str) -> bool {
    let line = line.to_lowercase();
    let starts_word =
        |at: usize| !line[..at].chars().next_back().is_some_and(char::is_alphanumeric);
    let ends_word = |at: usize| !line[at..].chars().next().is_some_and(char::is_alphanumeric);
    let found = |word: &str, whole: bool| {
        line.match_indices(word)
            .any(|(at, _)| starts_word(at) && (!whole || ends_word(at + word.len())))
    };
    SECRET_WORDS.iter().any(|word| found(word, false)) || found("pin", true)
}

#[cfg(test)]
mod tests;
