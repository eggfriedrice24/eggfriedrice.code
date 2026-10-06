//! The git dirs of a worktree or submodule project (the spec's section 5.1).
//!
//! The `.git` file of such a project and the `commondir` it leads to lie in or near the
//! writable project, so their text is not trusted at call time. `efr project add`
//! reads them once into a [`WorktreeRecord`], which efrd keeps in
//! `$S/sandbox/projects/` (masked in the sandbox). Before each call efrd reads them again
//! with [`check_worktree`]: only when they still match the record do the git dirs
//! become write roots; else git writes fail with `EROFS` and the tool result says to run
//! `efr project add` again.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::fs_view::{FileKind, FsView, resolve};
use crate::paths::normalize;

/// The most bytes of a `.git` file or a `commondir` file.
const MAX_LINK_FILE: usize = 4096;

/// Where a worktree project's git dirs are, as efrd read them at registration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRecord {
    /// The project root, which holds the `.git` file.
    pub root: PathBuf,
    /// The git dir that the `.git` file names, with its links followed.
    pub git_dir: PathBuf,
    /// The common dir that `commondir` names, or the git dir itself.
    pub common_dir: PathBuf,
}

impl WorktreeRecord {
    /// The record as the bytes of its file.
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec_pretty(self)
            .map_err(|source| SandboxError::Json { what: "worktree", source })
    }

    /// Reads a record file.
    pub fn from_json(bytes: &[u8]) -> Result<WorktreeRecord, SandboxError> {
        serde_json::from_slice(bytes)
            .map_err(|source| SandboxError::Json { what: "worktree", source })
    }
}

/// The file name of the record of the project at `root` below
/// `$S/sandbox/projects/`: the root's components joined by `%`, with `%` escaped.
pub fn record_file_name(root: &Path) -> String {
    format!("{}.json", crate::spec::layer_name(root))
}

/// What a check of a worktree project found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorktreeCheck {
    /// `.git` is a directory or missing: not a worktree project.
    NotWorktree,
    /// The `.git` file and `commondir` still name the recorded git dirs.
    Matches(WorktreeRecord),
    /// They name other dirs now: the git dirs are not write roots.
    Changed,
    /// No record exists: the project was registered before the sandbox.
    NoRecord,
}

/// The git dir that a `.git` file's text names: the path after `gitdir:` on its
/// first line, as written.
pub fn parse_git_file(text: &str) -> Option<PathBuf> {
    let line = text.lines().next()?;
    let path = line.strip_prefix("gitdir:")?.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Reads the worktree link of the project at `root`: `None` when `.git` is not a
/// file.
pub fn read_worktree(root: &Path, fs: &dyn FsView) -> Result<Option<WorktreeRecord>, SandboxError> {
    let dot_git = root.join(".git");
    match fs.lstat(&dot_git) {
        Ok(FileKind::File) => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(SandboxError::Io { path: dot_git, source }),
    }
    let text = read_text(fs, &dot_git)?;
    let Some(named) = parse_git_file(&text) else {
        return Err(SandboxError::Io {
            path: dot_git,
            source: io::Error::new(io::ErrorKind::InvalidData, "no gitdir line"),
        });
    };
    let git_dir = real_dir(fs, &root.join(named))?;
    let common_file = git_dir.join("commondir");
    let common_dir = match fs.lstat(&common_file) {
        Ok(FileKind::File) => {
            let text = read_text(fs, &common_file)?;
            real_dir(fs, &git_dir.join(text.trim()))?
        }
        _ => git_dir.clone(),
    };
    Ok(Some(WorktreeRecord { root: root.to_path_buf(), git_dir, common_dir }))
}

/// Reads the worktree link of `root` again and compares it with `record`.
pub fn check_worktree(
    root: &Path,
    record: Option<&WorktreeRecord>,
    fs: &dyn FsView,
) -> Result<WorktreeCheck, SandboxError> {
    let Some(now) = read_worktree(root, fs)? else {
        return Ok(WorktreeCheck::NotWorktree);
    };
    Ok(match record {
        None => WorktreeCheck::NoRecord,
        Some(record) if *record == now => WorktreeCheck::Matches(now),
        Some(_) => WorktreeCheck::Changed,
    })
}

fn read_text(fs: &dyn FsView, path: &Path) -> Result<String, SandboxError> {
    let bytes = fs
        .read_file(path, MAX_LINK_FILE)
        .map_err(|source| SandboxError::Io { path: path.to_path_buf(), source })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn real_dir(fs: &dyn FsView, path: &Path) -> Result<PathBuf, SandboxError> {
    let Some(path) = normalize(path) else {
        return Err(SandboxError::SpecPath { field: "git dir", path: path.to_path_buf() });
    };
    let resolved = resolve(fs, &path)?;
    if resolved.kind != Some(FileKind::Dir) {
        return Err(SandboxError::Io {
            path,
            source: io::Error::new(io::ErrorKind::NotFound, "the git dir is missing"),
        });
    }
    Ok(resolved.path)
}

#[cfg(test)]
mod tests;
