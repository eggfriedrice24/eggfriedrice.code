//! The git code of a git or gh line that runs outside the sandbox (v0.0.4, B4).
//!
//! An approved `git push` runs with the user's full rights, and git runs code that the
//! repository names: its hooks, the config keys that start a program, the programs
//! that these name, and the same of each submodule. When a sandboxed command can write
//! one of them, one approval would run planted code outside the sandbox. So efrd
//! checks the repository of each git and gh line that reaches a remote
//! ([`FactRequest::git_code`](crate::FactRequest)), and [`apply`] makes the exit one
//! that only the user may approve when a part is writable or when efrd could not check
//! it. Its notes name the file on the question.

use std::path::{Path, PathBuf};

use efr_protocol::{ExitKind, GitCodeFact};

use super::envelope::resolve;
use super::programs::base_name;
use super::scan::OPAQUE;
use super::{Approver, ExitNeed};
use crate::{CallFacts, GitCodeAsk, Locations};

/// The options of git before its subcommand that take the next word as their value.
const GIT_VALUES: &[&str] =
    &["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env", "--attr-source"];

/// The most notes about writable parts on one question; the rest is a count.
const MAX_NOTES: usize = 3;

/// What efrd checks for one simple command, as [`trigger`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Trigger {
    /// The command runs git in this repository.
    Ask(GitCodeAsk),
    /// The command runs git in a repository that efr cannot tell, such as one after a
    /// `cd` or behind a variable: efrd cannot check it.
    Unknown,
}

/// The git code check of the simple command `words` (its program first) that starts in
/// `dir`: a git command that reaches a remote (`push`, `send-pack`, `fetch`, `pull`,
/// `ls-remote`, `remote update`, `submodule update`), or a gh command that runs git in
/// the repository (`pr create`, `pr merge`, `pr checkout`, `repo sync`,
/// `repo create --push`, `repo fork --clone`). `None` for any other command.
pub(super) fn trigger(
    words: &[String],
    dir: Option<&Path>,
    locations: &Locations,
) -> Option<Trigger> {
    let (first, args) = words.split_first()?;
    match base_name(first) {
        "git" => git_trigger(args, dir, locations),
        "gh" => gh_runs_git(args).then(|| match dir {
            Some(dir) => Trigger::Ask(GitCodeAsk {
                dir: dir.to_path_buf(),
                git_dir: None,
                work_tree: None,
                recurse: gh_recurses(args),
            }),
            None => Trigger::Unknown,
        }),
        _ => None,
    }
}

fn git_trigger(args: &[String], dir: Option<&Path>, locations: &Locations) -> Option<Trigger> {
    let mut at = 0;
    let mut start: Option<PathBuf> = dir.map(Path::to_path_buf);
    let mut known = start.is_some();
    let mut git_dir: Option<String> = None;
    let mut work_tree: Option<String> = None;
    while let Some(word) = args.get(at) {
        let (option, inline) = match word.split_once('=') {
            Some((option, value)) if option.starts_with("--") => (option, Some(value)),
            _ => (word.as_str(), None),
        };
        if !option.starts_with('-') {
            break;
        }
        let takes_value = GIT_VALUES.contains(&option) && inline.is_none();
        let value = if takes_value { args.get(at + 1).map(String::as_str) } else { inline };
        match option {
            "-C" => match value {
                // NOTE: each `-C` is taken relative to the one before it, as git does.
                Some(value) => {
                    start = resolve(value, start.as_deref(), locations);
                    known &= start.is_some();
                }
                None => known = false,
            },
            "--git-dir" => git_dir = value.map(str::to_owned),
            "--work-tree" => work_tree = value.map(str::to_owned),
            _ => {}
        }
        at += if takes_value { 2 } else { 1 };
    }
    let sub = args.get(at)?.as_str();
    let rest = &args[at + 1..];
    let recurse = match sub {
        "push" | "send-pack" | "fetch" | "pull" => true,
        "ls-remote" => false,
        "remote"
            if rest.iter().find(|word| !word.starts_with('-')).is_some_and(|w| w == "update") =>
        {
            false
        }
        "submodule" if rest.iter().any(|word| word == "update") => true,
        _ => return None,
    };
    let Some(start) = start.filter(|_| known) else { return Some(Trigger::Unknown) };
    let placed = |word: Option<String>| -> Result<Option<PathBuf>, ()> {
        match word {
            None => Ok(None),
            Some(word) => resolve(&word, Some(&start), locations).map(Some).ok_or(()),
        }
    };
    let (Ok(git_dir), Ok(work_tree)) = (placed(git_dir), placed(work_tree)) else {
        return Some(Trigger::Unknown);
    };
    Some(Trigger::Ask(GitCodeAsk { dir: start, git_dir, work_tree, recurse }))
}

/// The group and the action of a gh command: its first two words that are no option,
/// past the value of `-R` and `--repo`.
fn gh_verbs(args: &[String]) -> (Option<&str>, Option<&str>) {
    let mut words = Vec::new();
    let mut at = 0;
    while let Some(word) = args.get(at) {
        if word == "-R" || word == "--repo" {
            at += 2;
            continue;
        }
        if !word.starts_with('-') {
            words.push(word.as_str());
        }
        at += 1;
    }
    (words.first().copied(), words.get(1).copied())
}

/// True for a gh command that runs git in the repository of its directory.
fn gh_runs_git(args: &[String]) -> bool {
    let flag = |name: &str| {
        args.iter().any(|word| {
            word == name || word.strip_prefix(name).is_some_and(|rest| rest.starts_with('='))
        })
    };
    match gh_verbs(args) {
        (Some("pr"), Some("create" | "merge" | "checkout")) => true,
        (Some("repo"), Some("sync")) => true,
        (Some("repo"), Some("create")) => flag("--push"),
        (Some("repo"), Some("fork")) => flag("--clone"),
        _ => false,
    }
}

/// True for a gh command that can reach the submodules: `repo sync`, `pr checkout`
/// with `--recurse-submodules`, and every push.
fn gh_recurses(args: &[String]) -> bool {
    !matches!(gh_verbs(args), (Some("pr"), Some("merge")))
}

/// What the facts say about the git code of a line, for [`apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CodeState<'f> {
    /// efrd checked every repository, and a contained call can write none of it.
    Clean,
    /// efrd could not check the repository of `dir`, or efr could not tell it (`None`).
    NotChecked(Option<PathBuf>),
    /// A contained call can write these parts.
    Writable(Vec<&'f GitCodeFact>),
}

/// What the facts say about the git code of a line whose checks are `triggers`.
pub(super) fn checked<'f>(triggers: &[Trigger], facts: &'f CallFacts) -> CodeState<'f> {
    let first_dir = || {
        triggers.iter().find_map(|trigger| match trigger {
            Trigger::Ask(ask) => Some(ask.work_tree.clone().unwrap_or_else(|| ask.dir.clone())),
            Trigger::Unknown => None,
        })
    };
    if triggers.contains(&Trigger::Unknown) {
        return CodeState::NotChecked(None);
    }
    let Some(code) = &facts.git_code else { return CodeState::NotChecked(first_dir()) };
    let writable: Vec<&GitCodeFact> = code.items.iter().filter(|item| item.writable).collect();
    if !writable.is_empty() {
        return CodeState::Writable(writable);
    }
    if !code.checked {
        return CodeState::NotChecked(first_dir());
    }
    CodeState::Clean
}

/// Makes each `outside` and `upload` exit of `needs` one that only the user may approve
/// when the git code of the line is not clean, with a note for each part that a
/// sandboxed command can write, or that says efr could not check it. `None` facts (no
/// sandbox in efrd) keep the route: no exit runs outside the sandbox without one.
pub(super) fn apply(
    needs: &mut [ExitNeed],
    triggers: &[Trigger],
    facts: Option<&CallFacts>,
    home: &Path,
) {
    if triggers.is_empty() {
        return;
    }
    let Some(facts) = facts else { return };
    let notes = match checked(triggers, facts) {
        CodeState::Clean => return,
        CodeState::NotChecked(dir) => vec![match dir {
            Some(dir) => format!("efr could not check the git code of {}", tilde(&dir, home)),
            None => "efr could not check the git code of the line's repository".to_owned(),
        }],
        CodeState::Writable(items) => writable_notes(&items, home),
    };
    for need in needs.iter_mut().filter(|need| runs_git_code(need.kind)) {
        need.approver = Approver::User;
        for note in &notes {
            if !need.notes.contains(note) {
                need.notes.push(note.clone());
            }
        }
    }
}

/// True for an exit kind that runs the line outside the sandbox with the user's rights.
fn runs_git_code(kind: ExitKind) -> bool {
    matches!(kind, ExitKind::Outside | ExitKind::Upload)
}

/// The notes of the writable parts `items`, at most [`MAX_NOTES`] and a count.
fn writable_notes(items: &[&GitCodeFact], home: &Path) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    for item in items {
        let note = writable_note(item, home);
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    if notes.len() > MAX_NOTES {
        let more = notes.len() - MAX_NOTES;
        notes.truncate(MAX_NOTES);
        notes.push(format!(
            "and {more} more part{} of the git code",
            if more == 1 { "" } else { "s" }
        ));
    }
    notes
}

/// The note of one writable part: `git runs <file>` for a hook or a program, `git
/// reads <key> from <file>` for a config key.
fn writable_note(item: &GitCodeFact, home: &Path) -> String {
    let file = tilde(&item.file, home);
    if is_config_key(&item.item) {
        format!("git reads {} from {file}, which a sandboxed command can write", item.item)
    } else {
        format!("git runs {file}, which a sandboxed command can write")
    }
}

/// True for an item that names a config key, such as `core.sshCommand`, not a hook
/// (`hooks/pre-push`) or a program (`ssh (PATH)`).
fn is_config_key(item: &str) -> bool {
    item.contains('.') && !item.contains(['/', ' '])
}

/// `path` with the home directory as `~`.
fn tilde(path: &Path, home: &Path) -> String {
    let shown = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    shown.replace(OPAQUE, "...")
}

#[cfg(test)]
mod tests;
