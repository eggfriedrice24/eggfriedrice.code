//! What efr must hold before a call so that `efr undo` can bring back what the call
//! destroys (efr's auto spec 10.7, phase B): the [`CoverAsk`]s of a destructive part
//! of a line.
//!
//! The engine only asks. The daemon answers each ask in [`CallFacts::covered`], and an
//! ask with no answer is never covered, so a lost fact keeps the question.

use std::path::{Path, PathBuf};

use super::programs::{self, base_name, is_sandbox_device};
use super::{Envelope, git_dir};
use crate::{AutoSupport, CallFacts, CoverAsk, Locations};

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
    let mut at = 0;
    let mut dirs = 0;
    while let Some(word) = args.get(at) {
        if word == "-C" {
            dirs += 1;
            at += 2;
        } else if GIT_PLAIN_OPTIONS.contains(&word.as_str()) {
            at += 1;
        } else if word.starts_with('-') {
            return Vec::new();
        } else {
            break;
        }
    }
    if dirs > 1 {
        return Vec::new();
    }
    let (Some(sub), Some(root)) = (args.get(at), git_dir(words, dir, locations)) else {
        return Vec::new();
    };
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

#[cfg(test)]
mod tests;
