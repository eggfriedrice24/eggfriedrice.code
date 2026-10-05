//! What is at the config file's path: nothing, a file, or a symbolic link and where it
//! points. `efr paths`, `efr config show` and `admin.status` report it, and the daemon
//! watches the directory of a link's target as well as its own.

use std::fs;
use std::path::{Path, PathBuf};

/// The config file as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileState {
    /// `config.toml` in the config root.
    pub path: PathBuf,
    /// True when `path` names a regular file, directly or through a symbolic link. A
    /// missing file is an empty config.
    pub exists: bool,
    /// What `path` points to when it is a symbolic link, as an absolute path: the end
    /// of the chain of links when it exists, else the link's own target.
    pub symlink_target: Option<PathBuf>,
}

impl FileState {
    /// Looks at `path`. This blocks; async code calls it in `spawn_blocking`.
    pub fn of(path: &Path) -> FileState {
        let exists = fs::metadata(path).is_ok_and(|meta| meta.is_file());
        let is_link = fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
        let symlink_target = is_link.then(|| target(path)).flatten();
        FileState { path: path.to_path_buf(), exists, symlink_target }
    }

    /// The directories whose changes can change the file: the one that holds `path`,
    /// and, for a link, the one that holds its target.
    pub fn watched_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for file in [Some(&self.path), self.symlink_target.as_ref()].into_iter().flatten() {
            if let Some(dir) = file.parent().filter(|dir| !dir.as_os_str().is_empty())
                && !dirs.iter().any(|known: &PathBuf| known == dir)
            {
                dirs.push(dir.to_path_buf());
            }
        }
        dirs
    }
}

/// The end of the link `path`, or its first target when the chain ends nowhere. A
/// relative target is taken from the link's directory.
fn target(path: &Path) -> Option<PathBuf> {
    if let Ok(resolved) = fs::canonicalize(path) {
        return Some(resolved);
    }
    let first = fs::read_link(path).ok()?;
    Some(match path.parent() {
        Some(dir) if first.is_relative() => dir.join(first),
        _ => first,
    })
}

#[cfg(test)]
mod tests;
