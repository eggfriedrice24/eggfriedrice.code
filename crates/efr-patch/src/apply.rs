//! A parsed patch applied to the current contents of its files.

use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

use crate::{Operation, Patch, PatchError};

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
    /// For an [`Added`](ChangeKind::Added) or [`Updated`](ChangeKind::Updated)
    /// content that is a file which a move of the patch brought from another path:
    /// the path that file had before the patch, so the caller can keep its mode.
    /// `None` for a new file, for a file that stays at `path`, and for a delete and a
    /// move (whose source is `path`).
    pub from: Option<PathBuf>,
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
    /// The file is removed from the change's path and `content` is written at `to`,
    /// where no file was.
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
/// same path folds into it. A path whose text ends as it started has no change.
///
/// Each hunk matches its file first exactly, then with tolerant passes: trailing
/// whitespace ignored, then all whitespace around each line ignored, then Unicode
/// punctuation (quotes, dashes, spaces) read as its ASCII form. A hunk that matches
/// nowhere is [`PatchError::NoMatch`] with the file, the hunk and the nearest lines; a
/// hunk without an anchor that matches in several places is
/// [`PatchError::Ambiguous`].
///
/// An add of a file that exists and a move onto a file that exists are
/// [`PatchError::Exists`]; an update, a delete or a move of a file that does not
/// exist is [`PatchError::Missing`]. A file that an earlier operation of the same
/// patch removed counts as absent, and one that it added counts as present.
pub fn apply(patch: &Patch, files: &dyn Files) -> Result<Vec<FileChange>, PatchError> {
    let mut state = State::new(files);
    for operation in &patch.operations {
        state.run(operation)?;
    }
    Ok(state.changes())
}

/// The files as the operations so far left them.
struct State<'f> {
    files: &'f dyn Files,
    /// Each path in the order the patch first names it.
    order: Vec<PathBuf>,
    /// The text of every path an operation changed; `None` when it removed the file.
    current: HashMap<PathBuf, Option<String>>,
    /// For a file that a move put at a path, the path it had before the patch.
    origin: HashMap<PathBuf, PathBuf>,
}

impl<'f> State<'f> {
    fn new(files: &'f dyn Files) -> Self {
        State { files, order: Vec::new(), current: HashMap::new(), origin: HashMap::new() }
    }

    fn text(&self, path: &Path) -> Option<&str> {
        match self.current.get(path) {
            Some(text) => text.as_deref(),
            None => self.files.text(path),
        }
    }

    fn name(&mut self, path: &Path) {
        if !self.order.iter().any(|named| named == path) {
            self.order.push(path.to_owned());
        }
    }

    /// Sets the text of `path`. The file there is a new one, unless `origin` says
    /// where it was before the patch.
    fn set(&mut self, path: &Path, text: Option<String>, origin: Option<PathBuf>) {
        self.current.insert(path.to_owned(), text);
        match origin {
            Some(origin) => self.origin.insert(path.to_owned(), origin),
            None => self.origin.remove(path),
        };
    }

    fn run(&mut self, operation: &Operation) -> Result<(), PatchError> {
        self.name(operation.path());
        if let Some(to) = operation.move_to() {
            self.name(to);
        }
        match operation {
            Operation::Add { path, content } => {
                if self.text(path).is_some() {
                    return Err(PatchError::Exists { path: path.clone() });
                }
                self.set(path, Some(content.clone()), None);
            }
            Operation::Delete { path } => {
                if self.text(path).is_none() {
                    return Err(PatchError::Missing { path: path.clone() });
                }
                self.set(path, None, None);
            }
            Operation::Update { path, move_to, hunks } => {
                let Some(text) = self.text(path) else {
                    return Err(PatchError::Missing { path: path.clone() });
                };
                let content = update::update(path, text, hunks)?;
                let origin = self.origin.get(path).cloned();
                match move_to.as_deref().filter(|to| to != path) {
                    None => self.set(path, Some(content), origin),
                    Some(to) => {
                        if self.text(to).is_some() {
                            return Err(PatchError::Exists { path: to.to_owned() });
                        }
                        self.set(path, None, None);
                        self.set(to, Some(content), Some(origin.unwrap_or_else(|| path.clone())));
                    }
                }
            }
        }
        Ok(())
    }

    /// The change of each path: its text before the patch against its text after.
    /// A file that left a path that had one, for a path that had none, is a move; any
    /// other move shows as a delete and an add or an update.
    fn changes(mut self) -> Vec<FileChange> {
        let mut moves: HashMap<PathBuf, PathBuf> = HashMap::new();
        for (to, from) in &self.origin {
            if self.files.text(from).is_some()
                && self.text(from).is_none()
                && self.files.text(to).is_none()
            {
                moves.insert(from.clone(), to.clone());
            }
        }
        let order = std::mem::take(&mut self.order);
        let mut changes = Vec::new();
        for path in order {
            if moves.values().any(|to| *to == path) {
                continue;
            }
            let before = self.files.text(&path);
            let after = match self.current.get_mut(&path) {
                Some(after) => after.take(),
                None => continue,
            };
            let from = self.origin.get(&path).filter(|origin| **origin != path).cloned();
            let kind = match (before, after, moves.get(&path)) {
                (Some(_), None, Some(to)) => {
                    let content =
                        self.current.get_mut(to).and_then(Option::take).unwrap_or_default();
                    ChangeKind::Moved { to: to.clone(), content }
                }
                (Some(_), None, None) => ChangeKind::Deleted,
                (None, Some(content), _) => ChangeKind::Added { content },
                (Some(before), Some(content), _) if before != content => {
                    ChangeKind::Updated { content }
                }
                _ => continue,
            };
            let from = match kind {
                ChangeKind::Added { .. } | ChangeKind::Updated { .. } => from,
                ChangeKind::Deleted | ChangeKind::Moved { .. } => None,
            };
            changes.push(FileChange { path, kind, from });
        }
        changes
    }
}

mod update;

#[cfg(test)]
mod tests;
