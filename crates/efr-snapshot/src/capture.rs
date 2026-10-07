//! One snapshot of one root: the tree of what is there now, written into the root's
//! store.
//!
//! A snapshot asks git once what differs from the persistent index (`ls-files` of the
//! modified, deleted and untracked files), so a root where nothing changed costs one
//! git run and the tree of the last snapshot comes back. New untracked files above the
//! size limit and nested repositories are left out. Only the changed paths are added,
//! literally ([`stage`]), and `write-tree` writes the tree when one is needed
//! ([`write_tree`]). With the ignored files of
//! `IgnoredFiles::Small`, a second `ls-files` lists the ignored files and directories,
//! and the small ones outside build and dependency directories are added too.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::SnapshotError;
use crate::runner::{Run, Runner};
use crate::store::Store;

/// The largest ignored file a snapshot takes.
pub const MAX_IGNORED_BYTES: u64 = 1024 * 1024;

/// The most ignored files that one snapshot adds, so a large ignored directory that
/// the skip list does not name cannot make it slow.
const MAX_IGNORED_FILES: usize = 2000;

/// Directories whose ignored files a snapshot never takes: builds and package
/// managers make them again.
pub const SKIPPED_DIRS: &[&str] =
    &["target", "node_modules", ".venv", "venv", "__pycache__", "dist", "build", ".next", ".cache"];

/// What a snapshot takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// New untracked files above this size are left out.
    pub max_file_bytes: u64,
    /// True takes small ignored files (up to 1 MiB, outside [`SKIPPED_DIRS`]) at the
    /// first and last snapshot of a turn.
    pub ignored_small: bool,
    /// A root with more files than this is not snapshotted.
    pub max_files: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_file_bytes: 10 * 1024 * 1024, ignored_small: true, max_files: 20_000 }
    }
}

/// What [`stage`] did to the persistent index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Staged {
    /// Nothing differed from the index.
    Unchanged,
    /// The index took the changes of the root.
    Changed,
    /// The root was not snapshotted, and why, for the debug log.
    Skipped(&'static str),
}

/// What `ls-files` said differs from the index.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Listing {
    /// Tracked files that changed or are gone.
    pub(crate) changed: BTreeSet<String>,
    /// Untracked files that are not ignored; a nested repository ends with `/`.
    pub(crate) others: Vec<String>,
}

/// Reads `ls-files -z -t` output: a tag, a space and a path per record.
pub(crate) fn parse_listing(out: &[u8]) -> Listing {
    let mut listing = Listing::default();
    for record in out.split(|byte| *byte == 0).filter(|record| record.len() > 2) {
        let Ok(text) = std::str::from_utf8(record) else {
            // NOTE: a name that is not UTF-8 cannot be passed back as a pathspec
            // through this crate's text, so it waits for a full snapshot.
            continue;
        };
        let (tag, path) = text.split_at(2);
        match tag {
            "? " => listing.others.push(path.to_owned()),
            _ => {
                listing.changed.insert(path.to_owned());
            }
        }
    }
    listing
}

/// Brings the persistent index of `store` up to its root with `limits`; `ignored`
/// also takes the small ignored files, and `fresh` (no tree known in this process)
/// copies the project's exclude file first.
pub(crate) async fn stage(
    runner: &Runner,
    store: &Store,
    limits: &Limits,
    ignored: bool,
    fresh: bool,
) -> Result<Staged, SnapshotError> {
    let root = store.root().to_path_buf();
    if !store.exists() {
        let prepared = store.clone();
        runner.blocking(store.dir(), move || prepared.make_dir()).await??;
        runner.init(store).await?;
        let prepared = store.clone();
        runner.blocking(store.dir(), move || prepared.finish_init()).await??;
    }
    if ignored || fresh {
        let copied = store.clone();
        runner.blocking(&root, move || copied.copy_exclude()).await??;
    }
    let stamped = store.clone();
    let now = SystemTime::from(runner.clock().now());
    runner.blocking(&root, move || stamped.stamp(now)).await?;

    let work = Run { work_tree: Some(&root), ..Run::default() };
    let listed = runner
        .checked(
            store,
            "ls-files",
            &["ls-files", "-z", "-t", "-m", "-d", "-o", "--exclude-standard"],
            work,
        )
        .await?;
    let listing = parse_listing(&listed);
    let entries = store.index_entries().unwrap_or(0);
    if entries + listing.others.len() as u64 > limits.max_files {
        return Ok(Staged::Skipped("the root has more files than snapshot.max_files"));
    }
    let max_file_bytes = limits.max_file_bytes;
    let others = listing.others;
    let base = root.clone();
    let others = runner.blocking(&root, move || keep_others(&base, others, max_file_bytes)).await?;
    let mut paths: Vec<String> = listing.changed.into_iter().collect();
    paths.extend(others);

    let mut forced = Vec::new();
    if ignored && limits.ignored_small {
        let listed = runner
            .checked(
                store,
                "ls-files",
                &["ls-files", "-z", "-o", "-i", "--exclude-standard", "--directory"],
                Run { work_tree: Some(&root), ..Run::default() },
            )
            .await?;
        let entries: Vec<String> = listed
            .split(|byte| *byte == 0)
            .filter_map(|entry| std::str::from_utf8(entry).ok())
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect();
        let base = root.clone();
        forced = runner.blocking(&root, move || small_ignored(&base, &entries)).await?;
    }

    if paths.is_empty() && forced.is_empty() {
        return Ok(Staged::Unchanged);
    }
    if !paths.is_empty() {
        let output = runner
            .run(
                store,
                &["add", "-A", "--ignore-errors", "--pathspec-from-file=-", "--pathspec-file-nul"],
                Run {
                    work_tree: Some(&root),
                    stdin: Some(nul_list(&paths)),
                    literal: true,
                    ..Run::default()
                },
            )
            .await?;
        if !output.success {
            // NOTE: with --ignore-errors git adds what it can read and fails for the
            // rest, such as a file that only root may read; the tree holds the rest.
            tracing::debug!(root = %root.display(), "some files could not be added to a snapshot");
        }
    }
    if !forced.is_empty() {
        let output = runner
            .run(
                store,
                &["add", "-f", "--ignore-errors", "--pathspec-from-file=-", "--pathspec-file-nul"],
                Run {
                    work_tree: Some(&root),
                    stdin: Some(nul_list(&forced)),
                    literal: true,
                    ..Run::default()
                },
            )
            .await?;
        if !output.success {
            tracing::debug!(root = %root.display(), "some ignored files could not be added to a snapshot");
        }
    }
    Ok(Staged::Changed)
}

/// Writes the tree of the persistent index of `store`.
pub(crate) async fn write_tree(runner: &Runner, store: &Store) -> Result<String, SnapshotError> {
    let tree = runner.checked(store, "write-tree", &["write-tree"], Run::default()).await?;
    let tree = String::from_utf8(tree)
        .map(|tree| tree.trim().to_owned())
        .map_err(|_| SnapshotError::BadOutput { command: "write-tree" })?;
    if tree.is_empty() || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SnapshotError::BadOutput { command: "write-tree" });
    }
    Ok(tree)
}

/// The paths of `list`, each followed by a NUL, for `--pathspec-from-file`.
fn nul_list(list: &[String]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(list.iter().map(|path| path.len() + 1).sum());
    for path in list {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// The new untracked files of `others` that a snapshot takes: no nested repository
/// (an entry that ends with `/`) and no regular file above `max_file_bytes`. A link
/// is taken as a link.
pub(crate) fn keep_others(root: &Path, others: Vec<String>, max_file_bytes: u64) -> Vec<String> {
    others
        .into_iter()
        .filter(|path| {
            if path.ends_with('/') {
                return false;
            }
            match fs::symlink_metadata(root.join(path)) {
                Ok(metadata) if metadata.is_file() => {
                    let keep = metadata.len() <= max_file_bytes;
                    if !keep {
                        tracing::debug!(path = %path, bytes = metadata.len(), "a large untracked file is left out of the snapshot");
                    }
                    keep
                }
                Ok(metadata) => !metadata.is_dir(),
                // NOTE: gone since git listed it; git add drops it again.
                Err(_) => true,
            }
        })
        .collect()
}

/// The ignored files that a snapshot takes from `entries` of `ls-files -o -i
/// --directory`: files up to [`MAX_IGNORED_BYTES`] and links, outside
/// [`SKIPPED_DIRS`]; an ignored directory (an entry that ends with `/`) is walked
/// without following links, up to [`MAX_IGNORED_FILES`] files in all.
pub(crate) fn small_ignored(root: &Path, entries: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    for entry in entries {
        if kept.len() >= MAX_IGNORED_FILES {
            break;
        }
        let relative = entry.trim_end_matches('/');
        if relative.is_empty() || in_skipped_dir(relative) {
            continue;
        }
        if entry.ends_with('/') {
            walk(root, PathBuf::from(relative), &mut kept, 0);
        } else if let Some(path) = take(root, relative) {
            kept.push(path);
        }
    }
    kept.truncate(MAX_IGNORED_FILES);
    kept
}

/// True when a part of `relative` is a skipped directory or a `.git`.
fn in_skipped_dir(relative: &str) -> bool {
    relative.split('/').any(|part| part == ".git" || SKIPPED_DIRS.contains(&part))
}

/// `relative` when it is a small file or a link.
fn take(root: &Path, relative: &str) -> Option<String> {
    let metadata = fs::symlink_metadata(root.join(relative)).ok()?;
    let small = metadata.is_file() && metadata.len() <= MAX_IGNORED_BYTES;
    (small || metadata.file_type().is_symlink()).then(|| relative.to_owned())
}

/// Adds the small files below the ignored directory `relative` to `kept`.
fn walk(root: &Path, relative: PathBuf, kept: &mut Vec<String>, depth: usize) {
    const MAX_DEPTH: usize = 16;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(root.join(&relative)) else { return };
    let mut names: Vec<_> = entries.filter_map(Result::ok).map(|entry| entry.file_name()).collect();
    names.sort();
    for name in names {
        if kept.len() >= MAX_IGNORED_FILES {
            return;
        }
        let Some(text) = name.to_str() else { continue };
        let child = relative.join(text);
        let Some(child_text) = child.to_str() else { continue };
        if in_skipped_dir(child_text) {
            continue;
        }
        match fs::symlink_metadata(root.join(&child)) {
            Ok(metadata) if metadata.is_dir() => {
                // NOTE: a nested repository is not captured (efr's auto spec, 10.2).
                if !root.join(&child).join(".git").exists() {
                    walk(root, child, kept, depth + 1);
                }
            }
            Ok(_) => {
                if let Some(path) = take(root, child_text) {
                    kept.push(path);
                }
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
