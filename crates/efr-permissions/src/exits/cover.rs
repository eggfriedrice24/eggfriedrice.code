//! What efr must hold before a call so that `efr undo` can bring back what the call
//! destroys or writes (efr's auto spec 10.7 and 10.8, phase B): the [`CoverAsk`]s of a
//! destructive part of a line, and the copies of a home write.
//!
//! The engine only asks. The daemon answers each ask in [`CallFacts::covered`], and an
//! ask with no answer is never covered, so a lost fact keeps the question.

use std::path::{Path, PathBuf};

use efr_protocol::ExitKind;

use super::programs::{self, base_name, is_sandbox_device};
use super::{Envelope, ExitNeed, WriteBind, existing_kind, git_dir};
use crate::command::git;
use crate::{AutoSupport, CallFacts, CoverAsk, Locations, TargetKind};

/// The asks of a destructive part whose paths are `targets`: one [`CoverAsk::Path`]
/// for each, or none when efr cannot name one of them. A part with no ask is never
/// covered.
pub(super) fn path_asks(targets: &[Option<PathBuf>]) -> Vec<CoverAsk> {
    let known: Option<Vec<CoverAsk>> =
        targets.iter().map(|target| target.clone().map(CoverAsk::Path)).collect();
    known.unwrap_or_default()
}

/// The asks of a command of `words` that the program tables call destructive
/// (`git reset --hard`, `dd of=`, `truncate`, `find -delete`, `shred`): what efr must
/// hold so that undo brings back what it destroys (spec 10.7). Empty for a form that
/// undo can never bring back, such as `shred` (the user asked to destroy the bytes),
/// `git clean -x`, `git reflog expire` and `git gc --prune`, and for a line where efr
/// cannot name every target. `assigned` is true when the segment sets a variable,
/// which can point git at another repository. `at` resolves a word as the line does.
pub(super) fn program_asks(
    words: &[String],
    assigned: bool,
    dir: Option<&Path>,
    locations: &Locations,
    at: &dyn Fn(&str) -> Option<PathBuf>,
) -> Vec<CoverAsk> {
    let Some(program) = words.first() else {
        return Vec::new();
    };
    match base_name(program) {
        "git" if assigned => Vec::new(),
        "git" => git_asks(words, dir, locations),
        // NOTE: `shred` overwrites the bytes on purpose; a copy in efr's store would
        // keep what the user asked to destroy.
        "shred" => Vec::new(),
        "dd" => {
            let targets: Vec<Option<PathBuf>> = programs::dd_targets(words)
                .into_iter()
                .map(at)
                .filter(|path| !path.as_deref().is_some_and(is_sandbox_device))
                .collect();
            path_asks(&targets)
        }
        _ => match programs::destructive_targets(words) {
            Some(found) if !found.is_empty() => {
                path_asks(&found.into_iter().map(at).collect::<Vec<_>>())
            }
            _ => Vec::new(),
        },
    }
}

/// The options of `git` itself that change nothing about the repository it works on.
const GIT_PLAIN_OPTIONS: &[&str] = &["--no-pager", "-P", "--no-optional-locks"];

/// The asks of a destructive `git` command of `words` (spec 10.7): the tracked files and
/// the refs for `reset --hard`, `checkout`, `switch`; the tracked files for `restore`;
/// the untracked files for `clean -f` without `-x` and `-X`; the refs for `branch -D`;
/// the one stash entry for `stash drop` and `stash clear`. Nothing for `reflog` and
/// `gc`, and nothing when an option of git itself other than one `-C` could point it at
/// another repository or work tree (`-c`, `--git-dir`, `--work-tree`).
fn git_asks(words: &[String], dir: Option<&Path>, locations: &Locations) -> Vec<CoverAsk> {
    let args = &words[1..];
    let parse = git::exact(args);
    let mut dirs = 0;
    for global in &parse.globals {
        if global.is("-C") {
            dirs += 1;
        } else if !GIT_PLAIN_OPTIONS.contains(&args[global.at].as_str()) {
            return Vec::new();
        }
    }
    if dirs > 1 {
        return Vec::new();
    }
    let (Some(at), Some(root)) = (parse.sub, git_dir(words, dir, locations)) else {
        return Vec::new();
    };
    let sub = &args[at];
    let rest = &args[at + 1..];
    match sub.as_str() {
        "reset" | "checkout" | "switch" => {
            vec![CoverAsk::Tracked(root.clone()), CoverAsk::Refs(root)]
        }
        "restore" => vec![CoverAsk::Tracked(root)],
        "clean" if rest.iter().any(|word| cleans_ignored(word)) => Vec::new(),
        "clean" => vec![CoverAsk::Untracked(root)],
        "branch" => vec![CoverAsk::Refs(root)],
        "stash" => vec![CoverAsk::Stash(root)],
        _ => Vec::new(),
    }
}

/// True for `-x`, `-X` or a cluster of short options that holds one of them: `git
/// clean` then removes ignored files, which a snapshot leaves out when they are large.
fn cleans_ignored(word: &str) -> bool {
    word.strip_prefix('-')
        .is_some_and(|letters| !letters.starts_with('-') && letters.contains(['x', 'X']))
}

/// True when `support` lets a destructive part run with no exit and every ask of
/// `asks` is covered: a fact says so and the engine's own check by name agrees
/// ([`Envelope::may_cover`]). False for a part with no ask.
pub(super) fn covers_all(
    asks: &[CoverAsk],
    envelope: &Envelope<'_>,
    facts: Option<&CallFacts>,
    support: &AutoSupport,
) -> bool {
    support.undo
        && !asks.is_empty()
        && asks.iter().all(|ask| {
            envelope.may_cover(ask) && facts.is_some_and(|facts| facts.coverage(ask).is_covered())
        })
}

/// The asks that the daemon answers for a write of `path` outside the snapshot roots:
/// the file itself, the directory itself, and its parent directory, which a contained
/// call may get whole ([`WriteBind::Parent`]). The parent is left out when it is the
/// home directory, above it, or a dir that many programs share, which no bind opens.
pub(super) fn write_asks(path: &Path, home: &Path) -> Vec<CoverAsk> {
    let mut asks = vec![CoverAsk::Home(path.to_path_buf()), CoverAsk::HomeDir(path.to_path_buf())];
    if let Some(parent) = path.parent()
        && !home.starts_with(parent)
        && !super::envelope::shared_home_dir(parent, home)
    {
        asks.push(CoverAsk::HomeDir(parent.to_path_buf()));
    }
    asks
}

/// What efrd copies into its home store before an exit of `kind` writes `path`, while
/// it keeps the store ([`AutoSupport::floor_backup`]): a file as itself, a directory
/// with what it holds, and for a bind of the parent ([`WriteBind::Parent`]) the parent
/// with what it holds, because the call can change any file there (spec 10.8). Nothing
/// for a secret, efr's own config and a device or socket. `kind_fact` is what a fact
/// says `path` is, `None` when no fact says.
pub(super) fn write_copies(
    kind: ExitKind,
    path: &Path,
    kind_fact: Option<Option<TargetKind>>,
    bind: Option<&WriteBind>,
    support: &AutoSupport,
) -> Vec<CoverAsk> {
    if !support.floor_backup || matches!(kind, ExitKind::Secret | ExitKind::Config) {
        return Vec::new();
    }
    let path = path.to_path_buf();
    let dir = kind == ExitKind::AboveRoot || kind_fact == Some(Some(TargetKind::Dir));
    match bind {
        Some(WriteBind::Target | WriteBind::MakeDir) => vec![CoverAsk::HomeDir(path)],
        Some(WriteBind::Parent(parent)) => {
            vec![CoverAsk::Home(path), CoverAsk::HomeDir(parent.clone())]
        }
        _ if kind_fact == Some(Some(TargetKind::Other)) => Vec::new(),
        _ if dir => vec![CoverAsk::HomeDir(path)],
        _ => vec![CoverAsk::Home(path)],
    }
}

/// Sets what efrd copies before the write of `need` ([`write_copies`] of its kind,
/// target and bind), [`ExitNeed::backup_before_write`] when that is anything, and
/// whether `facts` say that every copy is covered.
pub(crate) fn set_copies(need: &mut ExitNeed, facts: Option<&CallFacts>, support: &AutoSupport) {
    let Some(path) = need.target.clone() else { return };
    let kind_fact = existing_kind(facts, &path);
    need.copies = write_copies(need.kind, &path, kind_fact, need.bind.as_ref(), support);
    need.backup_before_write = !need.copies.is_empty();
    need.covered = covered(&need.copies, facts);
}

/// Whether the daemon covers every ask of `copies`, for the exit record: `None` when
/// there is nothing to copy, `Some(false)` when a fact is missing.
pub(super) fn covered(copies: &[CoverAsk], facts: Option<&CallFacts>) -> Option<bool> {
    (!copies.is_empty()).then(|| {
        copies.iter().all(|ask| facts.is_some_and(|facts| facts.coverage(ask).is_covered()))
    })
}

#[cfg(test)]
mod tests;
