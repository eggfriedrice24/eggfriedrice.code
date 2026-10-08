//! A parsed patch.

use std::path::{Path, PathBuf};

/// A parsed patch: its operations in the order the patch gives them.
///
/// Paths are as the patch writes them, relative or absolute; the caller resolves them
/// (with [`Patch::map_paths`]) before it reads the files and calls
/// [`apply`](crate::apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// The operations, in order. A patch that parses has at least one.
    pub operations: Vec<Operation>,
}

impl Patch {
    /// The same patch with every path, the target of a move too, replaced by
    /// `resolve(path)`, such as the path joined to the call's working directory.
    #[must_use]
    pub fn map_paths(self, mut resolve: impl FnMut(&Path) -> PathBuf) -> Patch {
        let operations = self
            .operations
            .into_iter()
            .map(|operation| match operation {
                Operation::Add { path, content } => {
                    Operation::Add { path: resolve(&path), content }
                }
                Operation::Delete { path } => Operation::Delete { path: resolve(&path) },
                Operation::Update { path, move_to, hunks } => Operation::Update {
                    path: resolve(&path),
                    move_to: move_to.map(|to| resolve(&to)),
                    hunks,
                },
            })
            .collect();
        Patch { operations }
    }

    /// Every path that the patch reads or writes, in the order the patch names them,
    /// each once: the path of each operation, then the target of a move.
    pub fn paths(&self) -> Vec<&Path> {
        let mut paths: Vec<&Path> = Vec::new();
        for operation in &self.operations {
            for path in [Some(operation.path()), operation.move_to()].into_iter().flatten() {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }
}

/// One file operation of a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// `*** Add File: <path>`: a new file with `content`, the text of its `+` lines,
    /// each ending with a newline.
    Add {
        /// The new file.
        path: PathBuf,
        /// Its whole content.
        content: String,
    },
    /// `*** Delete File: <path>`: removes the file.
    Delete {
        /// The file to remove.
        path: PathBuf,
    },
    /// `*** Update File: <path>`: changes the file by its hunks, and with
    /// `*** Move to: <path>` writes the result to `move_to` and removes `path`.
    Update {
        /// The file to change.
        path: PathBuf,
        /// Where the changed file goes, for a move.
        move_to: Option<PathBuf>,
        /// The changes, in the order they apply from the top of the file. A move may
        /// have none.
        hunks: Vec<Hunk>,
    },
}

impl Operation {
    /// The path the operation names first: the file it adds, deletes or updates.
    pub fn path(&self) -> &Path {
        match self {
            Operation::Add { path, .. }
            | Operation::Delete { path }
            | Operation::Update { path, .. } => path,
        }
    }

    /// The target of a move, for an update with `*** Move to:`.
    pub fn move_to(&self) -> Option<&Path> {
        match self {
            Operation::Update { move_to, .. } => move_to.as_deref(),
            Operation::Add { .. } | Operation::Delete { .. } => None,
        }
    }

    /// True for a delete and a move: they remove a file that undo cannot bring back
    /// yet, so the permission check asks for them in every mode.
    pub fn is_destructive(&self) -> bool {
        matches!(self, Operation::Delete { .. } | Operation::Update { move_to: Some(_), .. })
    }
}

/// One change of an update: lines to find and what replaces them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hunk {
    /// The text of the `@@ <text>` lines before the hunk's lines, in order, such as
    /// `impl Foo` and then `fn bar`. Each one narrows where the next is searched; a
    /// bare `@@` adds nothing.
    pub anchors: Vec<String>,
    /// The lines of the hunk, in order.
    pub lines: Vec<HunkLine>,
    /// True after `*** End of File`: the hunk's old lines end the file.
    pub end_of_file: bool,
}

impl Hunk {
    /// The lines the hunk expects in the file: its context and removed lines.
    pub fn old_lines(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(text) | HunkLine::Remove(text) => Some(text.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect()
    }

    /// The lines that replace them: its context and added lines.
    pub fn new_lines(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(text) | HunkLine::Add(text) => Some(text.as_str()),
                HunkLine::Remove(_) => None,
            })
            .collect()
    }
}

/// One line of a hunk, without its marker and its newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    /// ` <text>`: a line that stays.
    Context(String),
    /// `-<text>`: a line that goes.
    Remove(String),
    /// `+<text>`: a line that comes.
    Add(String),
}

#[cfg(test)]
mod tests;
