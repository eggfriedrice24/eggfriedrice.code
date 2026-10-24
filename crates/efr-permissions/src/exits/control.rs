//! ssh control sockets (v0.0.5, OQS17 of the v0.0.4 shell items).
//!
//! An ssh control socket is a logged-in session. A process that connects to it runs
//! commands on the remote host with no key and no password, and the host on the line
//! does not choose the session: `ssh -S sock other-host cmd` runs `cmd` on the host of
//! the master that made `sock`. So the naming rule of the reviewer cannot check such a
//! line, and only the user approves one:
//!
//! - [`makes_or_uses`] reads the line rule: an `ssh` or `autossh` with `-S`, `-M`, `-O`
//!   or an `-o` of [`CONTROL_KEYS`], an `scp`, `sftp` or `sshfs` with such an `-o`, and
//!   the ssh command of `rsync -e` or `--rsh`;
//! - [`matches_pattern`] reads a `ControlPath` of the user's ssh config, which efrd gives
//!   to [`Locations::with_ssh_control_path`](crate::Locations::with_ssh_control_path):
//!   a socket at such a place counts as a credential agent's socket.
//!
//! The DS13 sentence of the shell tool's description still tells the model not to
//! make or use a control socket.

/// The `-o` keys of ssh, in lower case, that make or use a control socket. The value
/// `none` of `ControlPath` and the value `no` of `ControlMaster` turn the socket off.
const CONTROL_KEYS: &[&str] = &["controlpath", "controlmaster", "controlpersist"];

/// The note of the question for an exit of a line that makes or uses an ssh control
/// socket.
pub(crate) const CONTROL_NOTE: &str = "it makes or uses an ssh control socket: a logged-in session that runs commands on its \
     host with no key";

/// The options of `ssh` that take a value as the next word (as `programs.rs` has them).
const SSH_VALUE_LETTERS: &str = "BbcDEeFIiJLlmOopQRSWw";

/// The options of `autossh` that take a value: those of `ssh`, and `-M port`.
const AUTOSSH_VALUE_LETTERS: &str = "BbcDEeFIiJLlMmOopQRSWw";

/// True when the simple command `words` makes or uses an ssh control socket: `ssh` or
/// `autossh` with `-S <path>`, `-M` (ssh only; for autossh it is a port), `-O <command>`
/// or `-o` with a key of [`CONTROL_KEYS`]; `scp`, `sftp` or `sshfs` with such an `-o`;
/// `rsync -e` or `--rsh` whose command does one of these. `-S none`, `ControlPath=none`
/// and `ControlMaster=no` make or use no socket.
pub(crate) fn makes_or_uses(words: &[String]) -> bool {
    let Some((first, args)) = words.split_first() else { return false };
    match super::programs::base_name(first) {
        program @ ("ssh" | "autossh") => ssh_form(program, args.iter().map(String::as_str)),
        "scp" | "sftp" | "sshfs" => o_values(args).any(control_option),
        "rsync" => rsh_commands(args).any(|command| {
            let mut words = command.split_whitespace().map(|word| word.trim_matches(['\'', '"']));
            match words.next().map(super::programs::base_name) {
                Some(program @ ("ssh" | "autossh")) => ssh_form(program, words),
                _ => false,
            }
        }),
        _ => false,
    }
}

/// The ssh form of [`makes_or_uses`] over the arguments `args` of `program`. ssh also
/// reads options after the destination, up to the remote command.
fn ssh_form<'a>(program: &str, args: impl Iterator<Item = &'a str>) -> bool {
    let values = if program == "autossh" { AUTOSSH_VALUE_LETTERS } else { SSH_VALUE_LETTERS };
    let mut words = args;
    let mut destination = false;
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|letters| !letters.is_empty()) else {
            if destination {
                break;
            }
            destination = true;
            continue;
        };
        let taken = letters.char_indices().find(|(_, c)| values.contains(*c));
        let flags = taken.map_or(letters, |(at, _)| &letters[..at]);
        if program == "ssh" && flags.contains('M') {
            return true;
        }
        let Some((at, letter)) = taken else { continue };
        let inline = &letters[at + letter.len_utf8()..];
        let value = if inline.is_empty() { words.next().unwrap_or_default() } else { inline };
        let used = match letter {
            'S' => !value.eq_ignore_ascii_case("none"),
            'O' => true,
            'o' => control_option(value),
            _ => false,
        };
        if used {
            return true;
        }
    }
    false
}

/// True when the ssh option `option`, as `-o` takes it (`Key=value` or `Key value`),
/// makes or uses a control socket. A value that only the shell knows counts.
fn control_option(option: &str) -> bool {
    let option = option.trim();
    let (key, value) = match option.split_once(['=', ' ', '\t']) {
        Some((key, value)) => (key, value.trim_start_matches(['=', ' ', '\t']).trim()),
        None => (option, ""),
    };
    let key = key.to_ascii_lowercase();
    if !CONTROL_KEYS.contains(&key.as_str()) {
        return false;
    }
    match key.as_str() {
        "controlpath" => !value.eq_ignore_ascii_case("none"),
        "controlmaster" => !value.eq_ignore_ascii_case("no"),
        _ => true,
    }
}

/// The values of each `-o` of `args`: `-o value`, `-ovalue`, and for sshfs each part of
/// a value that commas split.
fn o_values(args: &[String]) -> impl Iterator<Item = &str> {
    let mut found: Vec<&str> = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let value = if word == "-o" {
            words.next().map(String::as_str)
        } else {
            word.strip_prefix("-o").filter(|value| !value.is_empty())
        };
        found.extend(value.into_iter().flat_map(|value| value.split(',')));
    }
    found.into_iter()
}

/// The remote shell commands of `rsync`'s `args`: `-e cmd`, `-ecmd`, `--rsh=cmd` and
/// `--rsh cmd`.
fn rsh_commands(args: &[String]) -> impl Iterator<Item = &str> {
    let mut found: Vec<&str> = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let command = if word == "-e" || word == "--rsh" {
            words.next().map(String::as_str)
        } else if let Some(command) = word.strip_prefix("--rsh=") {
            Some(command)
        } else {
            word.strip_prefix("-e").filter(|command| !command.is_empty())
        };
        found.extend(command);
    }
    found.into_iter()
}

/// True when `path` matches the ssh `ControlPath` `pattern`, whose `~` the caller
/// expanded. Each `%` token (`%C`, `%h`, `%r@%h:%p`) and each `${NAME}` matches any run
/// of characters, also one with a `/`, so a pattern can only match more paths, which
/// only adds questions. `%%` is a `%`. A pattern of `none` matches nothing.
pub(crate) fn matches_pattern(pattern: &str, path: &str) -> bool {
    if pattern.eq_ignore_ascii_case("none") {
        return false;
    }
    let parts = literal_parts(pattern);
    let Some((first, rest)) = parts.split_first() else { return false };
    let Some(mut tail) = path.strip_prefix(first.as_str()) else { return false };
    let Some((last, middle)) = rest.split_last() else { return tail.is_empty() };
    for part in middle {
        match tail.find(part.as_str()) {
            Some(at) => tail = &tail[at + part.len()..],
            None => return false,
        }
    }
    tail.ends_with(last.as_str())
}

/// The literal parts of a `ControlPath` pattern between its tokens: `/tmp/%r@%h` gives
/// `["/tmp/", "@", ""]`. Two tokens in a row give an empty part between them.
fn literal_parts(pattern: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '%' => match chars.next() {
                Some('%') => parts.last_mut().into_iter().for_each(|part| part.push('%')),
                _ => parts.push(String::new()),
            },
            '$' if chars.peek() == Some(&'{') => {
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                }
                parts.push(String::new());
            }
            c => parts.last_mut().into_iter().for_each(|part| part.push(c)),
        }
    }
    parts
}

#[cfg(test)]
mod tests;
