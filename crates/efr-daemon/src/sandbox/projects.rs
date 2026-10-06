//! The worktree registration record (efr's auto spec, section 5.1).
//!
//! When `efr project add` registers a project whose `.git` is a file (a worktree or a
//! submodule), efrd reads the `gitdir:` line and `commondir` once, with their links
//! followed, and keeps them in `$S/sandbox/projects/<root>.json`, which every
//! contained call masks. Before each call efrd reads them again: only when they still
//! match the record do the git dirs become write roots. A planted `.git` file or
//! `commondir` can therefore never widen what a call writes.

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use efr_sandbox::{WorktreeCheck, WorktreeRecord, check_worktree, read_worktree, record_file_name};

use crate::sandbox::fs::DaemonFs;

/// The directory of the records below efr's state root.
pub(crate) const PROJECTS_DIR: &str = "sandbox/projects";

/// The most bytes of one record file.
const MAX_RECORD: usize = 64 * 1024;

/// What the tool result says when a worktree's `.git` file names other dirs than at
/// registration.
pub(crate) const WORKTREE_CHANGED: &str = "[efr: this worktree's .git file changed; run efr project add again to accept it. Git \
     writes fail in the sandbox until then.]";

/// What the tool result says for a worktree project that has no record.
pub(crate) const WORKTREE_NO_RECORD: &str = "[efr: this worktree project was registered before the sandbox knew its git dirs; run \
     efr project add again for it. Git writes fail in the sandbox until then.]";

/// The record file of the project at `root`.
pub(crate) fn record_path(state: &Path, root: &Path) -> PathBuf {
    state.join(PROJECTS_DIR).join(record_file_name(root))
}

/// Reads the worktree link of the project at `root` and keeps it, or removes an old
/// record when `.git` is no file now. It blocks.
pub(crate) fn register(state: &Path, root: &Path) -> io::Result<Option<WorktreeRecord>> {
    let path = record_path(state, root);
    let record = read_worktree(root, &DaemonFs).map_err(io::Error::other)?;
    match &record {
        Some(record) => {
            let dir = state.join(PROJECTS_DIR);
            fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
            let bytes = record.to_json().map_err(io::Error::other)?;
            efr_stdx::fs::write_atomic(&path, &bytes).map_err(io::Error::other)?;
        }
        None => match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        },
    }
    Ok(record)
}

/// The kept record of the project at `root`, if any. A record that cannot be read
/// counts as none: its project's git dirs then stay read-only. It blocks.
pub(crate) fn load(state: &Path, root: &Path) -> Option<WorktreeRecord> {
    let bytes = DaemonFs::read(&record_path(state, root), MAX_RECORD).ok()?;
    WorktreeRecord::from_json(&bytes).ok().filter(|record| record.root == root)
}

/// Reads the worktree link of `root` again and compares it with its record. A link
/// that cannot be read counts as changed. It blocks.
pub(crate) fn check(state: &Path, root: &Path) -> WorktreeCheck {
    let record = load(state, root);
    check_worktree(root, record.as_ref(), &DaemonFs).unwrap_or(WorktreeCheck::Changed)
}

#[cfg(test)]
mod tests;
