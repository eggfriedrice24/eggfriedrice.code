//! What differs between two trees of one store, or a tree and the store's index, from
//! `git diff-tree` or `git diff-index --cached` with `-z -M --raw --numstat`, and the
//! list of a call or a turn across its roots.

use std::fs;
use std::path::{Path, PathBuf};

use efr_protocol::{ChangeKind, FileChange, FileChanges};

/// The arguments of the comparison of two trees, before the two tree ids.
pub(crate) const DIFF_TREE: &[&str] =
    &["diff-tree", "-r", "-z", "-M", "--raw", "--numstat", "--no-ext-diff", "--no-textconv"];

/// The arguments of the comparison of a tree with the index, before the tree id; the
/// output has the form of [`DIFF_TREE`]'s.
pub(crate) const DIFF_INDEX: &[&str] =
    &["diff-index", "--cached", "-z", "-M", "--raw", "--numstat", "--no-ext-diff", "--no-textconv"];

/// One changed path of one root, relative to the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootChange {
    pub(crate) path: String,
    pub(crate) kind: ChangeKind,
    pub(crate) from: Option<String>,
    pub(crate) added: u32,
    pub(crate) removed: u32,
    pub(crate) binary: bool,
}

/// One raw record before its line counts are known.
#[derive(Debug)]
struct Raw {
    kind: ChangeKind,
    path: String,
    from: Option<String>,
    gitlink: bool,
}

/// Reads the output of [`DIFF_TREE`]: every raw record, then every numstat record in
/// the same order. A nested repository (a gitlink) is left out.
pub(crate) fn parse(out: &[u8]) -> Vec<RootChange> {
    let mut tokens = out
        .split(|byte| *byte == 0)
        .map(|token| String::from_utf8_lossy(token).into_owned())
        .peekable();
    let mut raws = Vec::new();
    while let Some(token) = tokens.next_if(|token| token.starts_with(':')) {
        let fields: Vec<&str> = token.split(' ').collect();
        let modes = (fields.first().copied().unwrap_or(""), fields.get(1).copied().unwrap_or(""));
        let status = fields.get(4).and_then(|status| status.chars().next()).unwrap_or('M');
        let gitlink = modes.0 == ":160000" || modes.1 == "160000";
        let (kind, from, path) = match status {
            'A' => (ChangeKind::Added, None, tokens.next().unwrap_or_default()),
            'D' => (ChangeKind::Deleted, None, tokens.next().unwrap_or_default()),
            'R' => {
                let from = tokens.next().unwrap_or_default();
                (ChangeKind::Renamed, Some(from), tokens.next().unwrap_or_default())
            }
            'C' => {
                // NOTE: copies need -C, which this crate does not pass; read one as a new
                // file at its second path.
                let _from = tokens.next();
                (ChangeKind::Added, None, tokens.next().unwrap_or_default())
            }
            _ => (ChangeKind::Modified, None, tokens.next().unwrap_or_default()),
        };
        raws.push(Raw { kind, path, from, gitlink });
    }
    let mut changes = Vec::new();
    for raw in raws {
        let Some(stat) = tokens.next() else { break };
        let mut parts = stat.splitn(3, '\t');
        let added = parts.next().unwrap_or("-");
        let removed = parts.next().unwrap_or("-");
        if raw.from.is_some() && parts.next().is_some_and(str::is_empty) {
            // A rename's numstat names both paths in the next two tokens.
            tokens.next();
            tokens.next();
        }
        if raw.gitlink {
            continue;
        }
        let binary = added == "-" && removed == "-";
        changes.push(RootChange {
            path: raw.path,
            kind: raw.kind,
            from: raw.from,
            added: added.parse().unwrap_or(0),
            removed: removed.parse().unwrap_or(0),
            binary,
        });
    }
    changes
}

/// Shows a deletion of a file that is still a regular file in `root` as a change with
/// no line counts: the snapshot left the file out because it grew past its size limit
/// (`capture::by_size`), so the store holds no new content to count.
pub(crate) fn mark_left_out(root: &Path, changes: &mut [RootChange]) {
    for change in changes.iter_mut().filter(|change| change.kind == ChangeKind::Deleted) {
        if fs::symlink_metadata(root.join(&change.path)).is_ok_and(|metadata| metadata.is_file()) {
            change.kind = ChangeKind::Modified;
            change.added = 0;
            change.removed = 0;
            change.binary = false;
        }
    }
}

/// The changes of one root with the prefix that shows its paths.
#[derive(Debug, Clone)]
pub(crate) struct Shown {
    pub(crate) root: PathBuf,
    pub(crate) shown: String,
    pub(crate) changes: Vec<RootChange>,
}

/// The list of a call or a turn: every root's changes with the root's prefix, sorted
/// and cut by [`FileChanges::from_files`]. A path that two nested roots both list is
/// kept once, from the deeper root. `None` when nothing changed.
pub(crate) fn merge(roots: Vec<Shown>) -> Option<FileChanges> {
    let mut seen: Vec<(PathBuf, FileChange)> = Vec::new();
    let mut roots = roots;
    // Deeper roots first, so their names win.
    roots.sort_by_key(|root| std::cmp::Reverse(root.root.components().count()));
    for root in roots {
        for change in root.changes {
            let absolute = root.root.join(&change.path);
            if seen.iter().any(|(path, _)| path == &absolute) {
                continue;
            }
            let file = FileChange {
                path: format!("{}{}", root.shown, change.path),
                kind: change.kind,
                from: change.from.map(|from| format!("{}{from}", root.shown)),
                added: change.added,
                removed: change.removed,
                binary: change.binary,
            };
            seen.push((absolute, file));
        }
    }
    let files: Vec<FileChange> = seen.into_iter().map(|(_, file)| file).collect();
    (!files.is_empty()).then(|| FileChanges::from_files(files))
}

/// The prefix that shows a path below `root`, as the protocol's
/// [`FileChange::path`] says: empty for the turn's project root, `$SCRATCH/` for the
/// conversation's scratch directory, `~/...` below the home directory, else the
/// absolute path, each with a final `/`.
pub fn shown_prefix(
    root: &Path,
    project: Option<&Path>,
    scratch: Option<&Path>,
    home: &Path,
) -> String {
    if project == Some(root) {
        return String::new();
    }
    if scratch == Some(root) {
        return "$SCRATCH/".to_owned();
    }
    match root.strip_prefix(home) {
        Ok(below) if below.as_os_str().is_empty() => "~/".to_owned(),
        Ok(below) => format!("~/{}/", below.display()),
        Err(_) => {
            let text = root.display().to_string();
            if text.ends_with('/') { text } else { format!("{text}/") }
        }
    }
}

#[cfg(test)]
mod tests;
