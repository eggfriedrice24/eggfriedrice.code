//! A parsed patch applied to the current contents of its files.

use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

use crate::{Patch, PatchError};

/// The current text of the files a patch names, as the caller read them.
///
/// The engine does no IO: the caller reads every path of
/// [`Patch::paths`](crate::Patch::paths) first, refuses what it must not edit (a path
/// through a link, a file over 16 MiB, a binary file), and hands the texts over, such
/// as in a map.
pub trait Files {
    /// The text of `path`, or `None` when no file exists there.
    fn text(&self, path: &Path) -> Option<&str>;
}

impl<S: BuildHasher> Files for HashMap<PathBuf, String, S> {
    fn text(&self, path: &Path) -> Option<&str> {
        self.get(path).map(String::as_str)
    }
}

impl Files for BTreeMap<PathBuf, String> {
    fn text(&self, path: &Path) -> Option<&str> {
        self.get(path).map(String::as_str)
    }
}

/// What a patch does to one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The path, as the patch names it (after [`Patch::map_paths`](crate::Patch::map_paths)).
    pub path: PathBuf,
    /// What happens to it.
    pub kind: ChangeKind,
}

/// What happens to the path of a [`FileChange`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    /// No file was there; `content` is written.
    Added {
        /// The new file's whole content.
        content: String,
    },
    /// The file stays where it is with `content`.
    Updated {
        /// The file's whole new content.
        content: String,
    },
    /// The file is removed.
    Deleted,
    /// The file is removed from the change's path and `content` is written at `to`.
    Moved {
        /// The new path.
        to: PathBuf,
        /// The whole content at the new path.
        content: String,
    },
}

/// Applies `patch` to the current texts of `files`, and returns what to write.
///
/// Nothing is written here: every new content is computed first, so the caller writes
/// either every change or, after the first error, none. The operations apply in order,
/// each to the result of the ones before it. The result has one [`FileChange`] per
/// path that changes, in the order the patch first names it; a later operation on the
/// same path folds into it.
///
/// Each hunk matches its file first exactly, then with tolerant passes: trailing
/// whitespace ignored, then all whitespace around each line ignored, then Unicode
/// punctuation (quotes, dashes, spaces) read as its ASCII form. A hunk that matches
/// nowhere is [`PatchError::NoMatch`] with the file, the hunk and the nearest lines.
pub fn apply(patch: &Patch, files: &dyn Files) -> Result<Vec<FileChange>, PatchError> {
    // NOTE: a stub until the engine is built; the contract is in the README.
    let _ = (patch, files);
    Err(PatchError::NotBuilt)
}

#[cfg(test)]
mod tests;
