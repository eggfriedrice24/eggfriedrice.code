//! [`Snapshots::covers`]: whether the turn's snapshot holds every file that a
//! destructive command could destroy, so that `efr undo` can bring it back.
//!
//! The check is positive: it takes the root's snapshot now (the turn's `pre` and its
//! mirror when the turn has none yet) and looks for each file in the fresh tree. It
//! never relies on the record of what a snapshot left out, which only names the
//! reason.

use std::fs;
use std::path::{Component, Path, PathBuf};

use efr_protocol::{ConversationId, TurnId};

use crate::capture::{self, LeftOut, LeftOutReason, Limits, MAX_IGNORED_BYTES};
use crate::project_git::ProjectGit;
use crate::runner::Run;
use crate::snapshots::{Missed, Root, Snap, Snapshots};
use crate::store::Store;
use crate::turn::View;

/// What a destructive command can destroy in one root.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RootCover {
    /// Every file at or below this path (`rm -r`, `dd of=`, `truncate`, `: >`, a
    /// delete or move of `apply_patch`).
    Path(PathBuf),
    /// Every tracked file of the project, with an index equal to `HEAD`
    /// (`git reset --hard`, `git checkout` that discards, `git restore`).
    Tracked,
    /// Every untracked file that the ignore rules do not name (`git clean -f`).
    Untracked,
    /// The project's branches, tags and `HEAD` (`git reset`, `git branch -D`).
    Refs,
    /// The project's stash, with one entry (`git stash drop`, `git stash clear`).
    Stash,
}

/// Whether the turn's snapshot covers one [`RootCover`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoverResult {
    /// The snapshot holds everything that the command can destroy.
    Covered,
    /// The snapshot left a file out, for this reason.
    LeftOut(LeftOutReason),
    /// The root has no snapshot in the turn.
    NoSnapshot,
    /// The turn's mirror does not hold the project's refs or the commit of `HEAD`.
    NoMirror,
    /// The path is not below the root.
    OutsideRoot,
    /// The path is the root itself.
    IsRoot,
    /// The path is in a git dir, which a snapshot never holds.
    GitInternal,
    /// The project's index holds content that is only staged.
    StagedOnly,
    /// The stash has more than one entry; the older ones live only in its reflog.
    StashEntries,
    /// A snapshot of the root failed in the turn.
    Failed,
}

impl Snapshots {
    /// Whether the snapshot of `root` in the turn `turn` covers each of `asks`, in
    /// their order. It snapshots the root first, so the answer is about the files as
    /// they are now; when the turn has no snapshot of the root yet, this one is its
    /// `pre` and takes the mirror. The caller holds the plan lock of the root, so no
    /// call of the turn writes in between.
    pub async fn covers(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Root,
        asks: &[RootCover],
        limits: Limits,
    ) -> Vec<CoverResult> {
        let all = |result: CoverResult| vec![result; asks.len()];
        let Some(root) = self.canonical(vec![root.clone()]).await.into_iter().next() else {
            return all(CoverResult::NoSnapshot);
        };
        let first = !self.in_turn(turn, root.path());
        let taken = self.snap(root.path(), &limits, true).await;
        let fresh = if first {
            self.start_root(conversation, turn, &root, taken, &limits).await
        } else {
            match taken {
                Ok(snap) => Some(snap),
                Err(Missed::TooLarge | Missed::Failed) => {
                    self.mark_failed(turn, root.path());
                    None
                }
            }
        };
        let view = self.view(turn, root.path());
        let (fresh, view) = match (fresh, view) {
            (_, View::TooLarge) => return all(CoverResult::LeftOut(LeftOutReason::RootTooLarge)),
            (_, View::Missing) => return all(CoverResult::NoSnapshot),
            (None, _) | (_, View::Failed) => return all(CoverResult::Failed),
            (Some(_), View::Kept { failed: true, .. }) => return all(CoverResult::Failed),
            (Some(fresh), View::Kept { mirror, .. }) => (fresh, mirror),
        };
        let mut results = Vec::with_capacity(asks.len());
        for ask in asks {
            let result = match ask {
                RootCover::Path(path) => self.covers_path(&root, &fresh, path, &limits).await,
                RootCover::Tracked => self.covers_tracked(&root, &fresh, &view, &limits).await,
                RootCover::Untracked => self.covers_untracked(&root, &fresh, &limits).await,
                RootCover::Refs => self.covers_refs(&view, false).await,
                RootCover::Stash => self.covers_refs(&view, true).await,
            };
            results.push(result);
        }
        results
    }

    async fn covers_path(
        &self,
        root: &Root,
        fresh: &Snap,
        path: &Path,
        limits: &Limits,
    ) -> CoverResult {
        let path = if path.is_absolute() { path.to_path_buf() } else { root.path().join(path) };
        let git = root.git().cloned();
        let base = root.path().to_path_buf();
        let max = limits.max_files;
        let walked = self
            .runner()
            .blocking(&path.clone(), move || walk_target(&base, &path, git.as_ref(), max))
            .await;
        let walked = match walked {
            Ok(Ok(walked)) => walked,
            Ok(Err(result)) => return result,
            Err(_) => return CoverResult::Failed,
        };
        let Some((below, files)) = walked else { return CoverResult::Covered };
        if files.is_empty() {
            return CoverResult::Covered;
        }
        let store = Store::new(self.dir(), root.path());
        let held = match self.tree_paths(&store, &fresh.tree, &below).await {
            Ok(held) => held,
            Err(()) => return CoverResult::Failed,
        };
        match files.iter().find(|file| held.binary_search(file).is_err()) {
            None => CoverResult::Covered,
            Some(missing) => {
                CoverResult::LeftOut(self.reason(root.path(), fresh, missing, limits).await)
            }
        }
    }

    async fn covers_tracked(
        &self,
        root: &Root,
        fresh: &Snap,
        view: &Option<crate::mirror::Mirror>,
        limits: &Limits,
    ) -> CoverResult {
        let Some(project) = self.project(view).await else { return CoverResult::NoMirror };
        match project.staged().await {
            Ok(false) => {}
            Ok(true) => return CoverResult::StagedOnly,
            Err(_) => return CoverResult::NoMirror,
        }
        let Ok(refs) = project.refs().await else { return CoverResult::NoMirror };
        let head = refs
            .head_commit
            .clone()
            .or_else(|| refs.head_ref.as_ref().and_then(|name| refs.refs.get(name).cloned()));
        let Some(head) = head else { return CoverResult::NoMirror };
        let store = Store::new(self.dir(), root.path());
        // NOTE: the mirror took HEAD's commit at the turn's first snapshot; a commit
        // made since is not in the store, so its list of files is not known here.
        let Ok(tracked) = self.tree_paths(&store, &head, &[]).await else {
            return CoverResult::NoMirror;
        };
        let Ok(held) = self.tree_paths(&store, &fresh.tree, &[]).await else {
            return CoverResult::Failed;
        };
        let missing: Vec<String> =
            tracked.into_iter().filter(|path| held.binary_search(path).is_err()).collect();
        let base = root.path().to_path_buf();
        let present = self
            .runner()
            .blocking(&base.clone(), move || {
                missing.into_iter().find(|path| fs::symlink_metadata(base.join(path)).is_ok())
            })
            .await;
        match present {
            Ok(None) => CoverResult::Covered,
            Ok(Some(path)) => {
                CoverResult::LeftOut(self.reason(root.path(), fresh, &path, limits).await)
            }
            Err(_) => CoverResult::Failed,
        }
    }

    async fn covers_untracked(&self, root: &Root, fresh: &Snap, limits: &Limits) -> CoverResult {
        let store = Store::new(self.dir(), root.path());
        let listed = self
            .runner()
            .checked(
                &store,
                "ls-files",
                &["ls-files", "-z", "-o", "--exclude-standard"],
                Run { work_tree: Some(root.path()), ..Run::default() },
            )
            .await;
        let Ok(listed) = listed else { return CoverResult::Failed };
        // NOTE: a nested repository ends with `/`; `git clean` without `-ff` keeps it.
        let first = listed
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty() && !entry.ends_with(b"/"))
            .map(|entry| String::from_utf8_lossy(entry).into_owned())
            .next();
        match first {
            None => CoverResult::Covered,
            Some(path) => {
                CoverResult::LeftOut(self.reason(root.path(), fresh, &path, limits).await)
            }
        }
    }

    async fn covers_refs(&self, view: &Option<crate::mirror::Mirror>, stash: bool) -> CoverResult {
        if view.as_ref().is_none_or(|mirror| mirror.missing.is_some()) {
            return CoverResult::NoMirror;
        }
        let Some(project) = self.project(view).await else { return CoverResult::NoMirror };
        if !stash {
            return CoverResult::Covered;
        }
        let Ok(refs) = project.refs().await else { return CoverResult::NoMirror };
        if !refs.refs.contains_key("refs/stash") {
            return CoverResult::Covered;
        }
        match project.stash_entries().await {
            Ok(0 | 1) => CoverResult::Covered,
            Ok(_) => CoverResult::StashEntries,
            Err(_) => CoverResult::NoMirror,
        }
    }

    /// git on the project of a root whose first snapshot pinned its git dirs.
    async fn project(&self, view: &Option<crate::mirror::Mirror>) -> Option<ProjectGit> {
        let (dirs, pins) = view.as_ref()?.git.clone()?;
        let runner = self.runner().clone();
        let opened = self
            .runner()
            .blocking(&dirs.git_dir.clone(), move || ProjectGit::open(&runner, &dirs, Some(pins)))
            .await;
        opened.ok()?.ok()
    }

    /// The paths of the blobs and links of `tree` in `store`, at or below the paths of
    /// `below` (all of them when it is empty), sorted.
    async fn tree_paths(
        &self,
        store: &Store,
        tree: &str,
        below: &[String],
    ) -> Result<Vec<String>, ()> {
        let mut args = vec!["ls-tree", "-r", "-z", "--full-tree", tree];
        if !below.is_empty() {
            args.push("--");
            args.extend(below.iter().map(String::as_str));
        }
        let out = self
            .runner()
            .checked(store, "ls-tree", &args, Run { literal: true, ..Run::default() })
            .await
            .map_err(|_| ())?;
        let mut paths: Vec<String> = out
            .split(|byte| *byte == 0)
            .filter_map(|record| {
                let text = std::str::from_utf8(record).ok()?;
                let (meta, path) = text.split_once('\t')?;
                (meta.split(' ').nth(1) == Some("blob")).then(|| path.to_owned())
            })
            .collect();
        paths.sort();
        Ok(paths)
    }

    /// Why the fresh snapshot of `root` lacks `path`: from its record when it names the
    /// path or a directory above it, else from the file itself.
    async fn reason(
        &self,
        root: &Path,
        fresh: &Snap,
        path: &str,
        limits: &Limits,
    ) -> LeftOutReason {
        if let Some(found) = recorded(&fresh.left_out, path) {
            return found;
        }
        let file = root.join(path);
        let limits = *limits;
        let relative = path.to_owned();
        let derived = self
            .runner()
            .blocking(&file.clone(), move || {
                let bytes = fs::symlink_metadata(&file).map(|metadata| metadata.len()).unwrap_or(0);
                if capture::in_skipped_dir(&relative) {
                    LeftOutReason::SkippedDir
                } else if bytes > limits.max_file_bytes {
                    LeftOutReason::TooLarge
                } else if bytes > MAX_IGNORED_BYTES {
                    LeftOutReason::IgnoredTooLarge
                } else if !limits.ignored_small {
                    LeftOutReason::IgnoredOff
                } else {
                    LeftOutReason::Unreadable
                }
            })
            .await;
        derived.unwrap_or(LeftOutReason::Unreadable)
    }
}

/// The reason that `left_out` gives for `path` or a directory above it.
pub(crate) fn recorded(left_out: &[LeftOut], path: &str) -> Option<LeftOutReason> {
    left_out
        .iter()
        .find(|left| {
            left.path == path
                || left.path.is_empty()
                || path.strip_prefix(&left.path).is_some_and(|rest| rest.starts_with('/'))
        })
        .map(|left| left.reason)
}

/// What [`walk_target`] found: the paths relative to the root that a command can
/// destroy (the pathspecs of the tree listing), and every file at or below them.
type Walked = (Vec<String>, Vec<String>);

/// The paths relative to `root` that a command on `target` can destroy, and every file
/// at or below them (regular files and links, sorted), with an `lstat` walk that
/// follows no link; `None` when `target` does not exist; `Err` with the answer when the
/// path itself decides it. A link at the end of `target` counts with what it names:
/// `rm` removes the link, but `: >`, `truncate`, `dd of=` and `rm -r link/` write
/// through it, so both must be in the snapshot, and what it names must lie in the root.
fn walk_target(
    root: &Path,
    target: &Path,
    git: Option<&crate::project_git::GitDirs>,
    max_files: u64,
) -> Result<Option<Walked>, CoverResult> {
    let target = resolve(target);
    let relative = checked_relative(root, &target, git)?;
    let Ok(metadata) = fs::symlink_metadata(&target) else { return Ok(None) };
    if metadata.file_type().is_symlink() {
        // NOTE: a link that names nothing makes a new file when a command writes
        // through it, which destroys nothing.
        let Ok(real) = fs::canonicalize(&target) else {
            return Ok(Some((vec![relative.clone()], vec![relative])));
        };
        let (mut below, mut files): Walked =
            walk_target(root, &real, git, max_files)?.unwrap_or_default();
        below.push(relative.clone());
        files.push(relative);
        files.sort();
        files.dedup();
        if files.len() as u64 > max_files {
            return Err(CoverResult::LeftOut(LeftOutReason::RootTooLarge));
        }
        return Ok(Some((below, files)));
    }
    let mut files = Vec::new();
    if !metadata.is_dir() {
        if metadata.is_file() {
            files.push(relative.clone());
        }
        return Ok(Some((vec![relative], files)));
    }
    let mut stack = vec![target.clone()];
    while let Some(dir) = stack.pop() {
        if dir != target && fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Err(CoverResult::LeftOut(LeftOutReason::NestedRepo));
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            return Err(CoverResult::LeftOut(LeftOutReason::Unreadable));
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            // NOTE: a snapshot never holds a fifo, a socket or a device, and undo
            // never makes one; the docs name them.
            if !(kind.is_file() || kind.is_symlink()) {
                continue;
            }
            let Some(text) = path.strip_prefix(root).ok().and_then(Path::to_str) else {
                return Err(CoverResult::LeftOut(LeftOutReason::NotUtf8));
            };
            files.push(text.to_owned());
            if files.len() as u64 > max_files {
                return Err(CoverResult::LeftOut(LeftOutReason::RootTooLarge));
            }
        }
    }
    files.sort();
    Ok(Some((vec![relative], files)))
}

/// `target` relative to `root`, or the answer when the path itself decides it: outside
/// the root, the root itself, in a git dir, or a name that is not UTF-8.
fn checked_relative(
    root: &Path,
    target: &Path,
    git: Option<&crate::project_git::GitDirs>,
) -> Result<String, CoverResult> {
    let in_git = git.is_some_and(|dirs| {
        [&dirs.git_dir, &dirs.common_dir].iter().any(|dir| {
            let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            target.starts_with(dir)
        })
    });
    if in_git {
        return Err(CoverResult::GitInternal);
    }
    let Ok(relative) = target.strip_prefix(root) else { return Err(CoverResult::OutsideRoot) };
    if relative.as_os_str().is_empty() {
        return Err(CoverResult::IsRoot);
    }
    if relative.components().any(|part| part.as_os_str() == ".git") {
        return Err(CoverResult::GitInternal);
    }
    relative.to_str().map(str::to_owned).ok_or(CoverResult::LeftOut(LeftOutReason::NotUtf8))
}

/// `path` with `.` and `..` taken out and its parent directory resolved, so it
/// compares with the canonical root; the last part stays as it is, and
/// [`walk_target`] looks at a link there and at what it names.
fn resolve(path: &Path) -> PathBuf {
    let mut plain = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                plain.pop();
            }
            Component::CurDir => {}
            other => plain.push(other),
        }
    }
    match (plain.parent(), plain.file_name()) {
        (Some(parent), Some(name)) => {
            fs::canonicalize(parent).map_or_else(|_| plain.clone(), |dir| dir.join(name))
        }
        _ => plain,
    }
}

#[cfg(test)]
mod tests;
