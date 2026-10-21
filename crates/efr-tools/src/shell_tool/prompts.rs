//! The soft flag of a shell call (`may_prompt`): a line that may ask for a password or a
//! question at the terminal although no program of [`INTERACTIVE`] runs it, such as
//! `rsync` to another host, `scp` or `git` over ssh.
//!
//! The hard flag (`interactive`) asks in every mode outside `auto` and refuses a job. The
//! soft flag does neither: it only lets a call outside the sandbox run past the model's
//! timeout while a person who can answer follows the turn, and it makes the question say
//! that the user may need to type a password. A job of `git clone` with no terminal makes
//! ssh fail fast, which is right for a job.
//!
//! Pure: the words of the line, no IO.

use super::words::{Line, Word, is_assignment};
use super::{GIT_VALUE_OPTIONS, INTERACTIVE, WRAPPERS, git_subcommand, programs};

/// Programs that ask for a password or a host key whenever they run.
const PROMPTING: &[&str] = &["scp", "sftp", "sshfs", "ssh-copy-id", "mosh", "autossh", "ssh-add"];

/// The programs that the coarse scan of a line that the split cannot read whole flags by
/// name alone.
const BY_NAME: &[&str] =
    &["rsync", "scp", "sftp", "sshfs", "ssh-copy-id", "mosh", "autossh", "ssh-add", "ssh-keygen"];

/// The assignments that give `git` a program to run for ssh.
const GIT_SSH_VARIABLES: &[&str] = &["GIT_SSH_COMMAND", "GIT_SSH"];

/// True when the line `text`, split as `line`, may ask for a password or a question at
/// the terminal through a program that [`INTERACTIVE`] does not hold.
pub(super) fn may_prompt(line: &Line, text: &str) -> bool {
    line.commands.iter().any(|command| command_prompts(&command.words))
        || git_prompts(text)
        || (line.opaque && programs(text).iter().any(|program| BY_NAME.contains(program)))
}

/// True when the simple command `words` (assignments first) may ask, past its leading
/// assignments and wrappers such as `env` or `nice`.
fn command_prompts(words: &[Word]) -> bool {
    let mut assignments: Vec<&str> = Vec::new();
    let mut rest = words;
    loop {
        let skipped = rest.iter().take_while(|word| is_assignment(&word.text)).count();
        assignments.extend(rest[..skipped].iter().map(|word| word.text.as_str()));
        rest = &rest[skipped..];
        let Some((first, args)) = rest.split_first() else {
            return false;
        };
        let program = first.text.rsplit('/').next().unwrap_or(&first.text);
        let args: Vec<&str> = args.iter().map(|word| word.text.as_str()).collect();
        let looks_up = program == "command" && args.iter().any(|arg| matches!(*arg, "-v" | "-V"));
        if !WRAPPERS.contains(&program) || looks_up {
            return program_prompts(program, &args, &assignments);
        }
        // NOTE: the options of a wrapper and the assignments of `env` come before the
        // program that it runs, as `programs_of` reads them.
        let options = args.iter().take_while(|arg| arg.starts_with('-')).count();
        rest = &rest[1 + options..];
    }
}

/// True when `program` with `args`, after the assignments `assignments`, may ask.
fn program_prompts(program: &str, args: &[&str], assignments: &[&str]) -> bool {
    match program {
        "rsync" => {
            args.iter().any(|arg| is_remote(arg))
                || rsync_shell(args).is_some_and(remote_shell)
                || assigned(assignments, &["RSYNC_RSH"]).any(remote_shell)
        }
        "git" => {
            assigned(assignments, GIT_SSH_VARIABLES).any(remote_shell)
                || git_ssh_command(args).is_some_and(remote_shell)
        }
        // NOTE: `-N` gives the passphrase on the line, so nothing is asked.
        "ssh-keygen" => !args.iter().any(|arg| short_option(arg, 'N')),
        _ => PROMPTING.contains(&program),
    }
}

/// The values of the assignments of `assignments` to one of `names`.
fn assigned<'a>(assignments: &'a [&str], names: &'a [&str]) -> impl Iterator<Item = &'a str> {
    assignments.iter().filter_map(move |assignment| {
        let (name, value) = assignment.split_once('=')?;
        names.contains(&name).then_some(value)
    })
}

/// True when a remote shell `value` (the `-e` of `rsync`, `GIT_SSH_COMMAND`) runs `ssh`
/// or another program that may wait for input at the terminal. Its programs never give
/// the hard flag: `rsync -e 'sudo ssh'` stays soft, because the hard reasons come from
/// the line itself.
fn remote_shell(value: &str) -> bool {
    programs(value).iter().any(|program| *program == "ssh" || INTERACTIVE.contains(program))
}

/// The remote shell of `rsync` with `args`: `-e X`, `-eX` (also at the end of a group of
/// short options, such as `-avze ssh`), `--rsh=X` or `--rsh X`.
fn rsync_shell<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if *word == "--" {
            return None;
        }
        if let Some(value) = word.strip_prefix("--rsh=") {
            return Some(value);
        }
        if *word == "--rsh" {
            return words.next().copied();
        }
        let Some(letters) = word.strip_prefix('-').filter(|letters| !letters.starts_with('-'))
        else {
            continue;
        };
        if let Some(at) = letters.find('e') {
            let value = &letters[at + 1..];
            return if value.is_empty() { words.next().copied() } else { Some(value) };
        }
    }
    None
}

/// The value of `-c core.sshCommand=X` of `git` with `args`, before its subcommand. The
/// section and the key are read without case, as git reads them.
fn git_ssh_command<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let setting = match *word {
            "-c" => words.next().copied(),
            word => word.strip_prefix("-c").filter(|rest| rest.contains('=')),
        };
        if let Some((key, value)) = setting.and_then(|setting| setting.split_once('='))
            && key.eq_ignore_ascii_case("core.sshCommand")
        {
            return Some(value);
        }
        if *word != "-c" && GIT_VALUE_OPTIONS.contains(word) {
            words.next();
        } else if !word.starts_with('-') {
            return None;
        }
    }
    None
}

/// True when a `git` command in `text` reaches a remote in a form that the `network`
/// flag does not read: `remote update`, `archive --remote` and `send-email`. The forms of
/// `GIT_NETWORK` count too.
fn git_prompts(text: &str) -> bool {
    super::networked_git(text)
        || super::segments(text).any(|segment| match git_subcommand(segment) {
            Some(("remote", mut rest)) => {
                rest.find(|word| !word.starts_with('-')) == Some("update")
            }
            Some(("archive", mut rest)) => {
                rest.any(|word| word == "--remote" || word.starts_with("--remote="))
            }
            Some(("send-email", _)) => true,
            _ => false,
        })
}

/// True when `word` is a group of short options that holds `letter`, such as `-qN`.
fn short_option(word: &str, letter: char) -> bool {
    word.strip_prefix('-')
        .is_some_and(|letters| !letters.starts_with('-') && letters.contains(letter))
}

/// True for an operand on another host: `host:path`, `user@host:path`, `host::module` or
/// `rsync://`.
// NOTE: a copy of `is_remote` in `efr-permissions/src/exits/programs.rs`, which this
// crate cannot name; `remote_rows_match_the_classifier` in both crates holds the same
// rows.
fn is_remote(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }
    if arg.starts_with("rsync://") {
        return true;
    }
    match arg.find(':') {
        Some(colon) => !arg[..colon].contains('/') && colon > 0,
        None => false,
    }
}

#[cfg(test)]
mod tests;
