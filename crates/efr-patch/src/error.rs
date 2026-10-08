//! What can go wrong with a patch.

use std::path::PathBuf;

/// Why a patch cannot be parsed or applied, or a replacement cannot be made.
///
/// Each variant carries what the caller needs to tell the model how to try again: the
/// line of the patch, the file and the hunk, and the lines of the file nearest to what
/// did not match. The `Display` text is one sentence; `efr-tools` writes the longer
/// text the model reads from the fields.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PatchError {
    /// The text is not a patch.
    #[error("line {line} of the patch is wrong: {problem}")]
    Parse {
        /// The line of the patch, from 1.
        line: usize,
        /// What is wrong there.
        problem: ParseProblem,
    },

    /// An update, a delete or a move names a file that does not exist.
    #[error("{} does not exist", path.display())]
    Missing {
        /// The file.
        path: PathBuf,
    },

    /// An add names a file that exists, or a move goes to a file that exists.
    #[error("{} already exists", path.display())]
    Exists {
        /// The file.
        path: PathBuf,
    },

    /// An `@@ <text>` line of a hunk matches no line of its file after the hunks
    /// before it, after every tolerant pass.
    #[error("the `@@ {anchor}` line of hunk {hunk} matches no line of {}", path.display())]
    NoAnchor {
        /// The file.
        path: PathBuf,
        /// The hunk, from 1 among the hunks of its update.
        hunk: usize,
        /// The text of the `@@` line.
        anchor: String,
        /// The lines of the file nearest to the anchor, at most a few.
        nearest: Vec<NearLine>,
    },

    /// A hunk of an update matches nowhere in its file, after every tolerant pass.
    #[error("hunk {hunk} does not match the lines of {}", path.display())]
    NoMatch {
        /// The file.
        path: PathBuf,
        /// The hunk, from 1 among the hunks of its update.
        hunk: usize,
        /// The lines of the file nearest to the hunk's old lines, at most a few.
        nearest: Vec<NearLine>,
    },

    /// A hunk without an `@@ <text>` line matches more than one place after the
    /// hunks before it, so the engine cannot tell which one the patch means.
    #[error("hunk {hunk} matches {} places in {}", lines.len(), path.display())]
    Ambiguous {
        /// The file.
        path: PathBuf,
        /// The hunk, from 1 among the hunks of its update.
        hunk: usize,
        /// The first line of each place it matches, from 1.
        lines: Vec<usize>,
    },

    /// The old text of a replacement does not occur.
    #[error("the old text does not occur")]
    NotFound {
        /// The lines of the text nearest to the old text, at most a few.
        nearest: Vec<NearLine>,
    },

    /// The old text of a unique replacement occurs more than once.
    #[error("the old text occurs {count} times, not once")]
    NotUnique {
        /// How often it occurs.
        count: usize,
    },

    /// The old text of a replacement is empty.
    #[error("the old text is empty")]
    EmptyOld,
}

/// What is wrong with a line of a patch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseProblem {
    /// The first line is not `*** Begin Patch`.
    #[error("the patch must start with `*** Begin Patch`")]
    NoBegin,
    /// The last line is not `*** End Patch`.
    #[error("the patch must end with `*** End Patch`")]
    NoEnd,
    /// Text follows an `*** End Patch` line.
    #[error("no text can follow `*** End Patch`")]
    AfterEnd,
    /// The patch has no operation between its first and last line.
    #[error("the patch has no file operation")]
    Empty,
    /// The line starts no operation and belongs to none.
    #[error("expected `*** Add File:`, `*** Delete File:` or `*** Update File:`")]
    NotAnOperation,
    /// An operation names no path.
    #[error("the operation names no path")]
    NoPath,
    /// A line of an added file does not start with `+`.
    #[error("each line of an added file must start with `+`")]
    NotAnAddLine,
    /// An update has no hunk and no move.
    #[error("the update has no hunk")]
    EmptyUpdate,
    /// A hunk has `@@` lines but no line with ` `, `-` or `+`.
    #[error("the hunk has no line with ` `, `-` or `+`")]
    EmptyHunk,
    /// A line of a hunk starts with none of ` `, `-` and `+`.
    #[error("a line of a hunk must start with ` `, `-` or `+`")]
    NotAHunkLine,
    /// `*** Move to:` is not the line right after `*** Update File:`, or comes twice.
    #[error("`*** Move to:` must be the line right after `*** Update File:`")]
    MisplacedMove,
    /// A line after `*** End of File` starts no new hunk and no new operation.
    #[error("after `*** End of File`, expected `@@` or a new operation")]
    AfterEndOfFile,
}

/// One line of a file, for an error that shows where a hunk or an old text nearly
/// matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearLine {
    /// The line number, from 1.
    pub number: usize,
    /// The line, without its newline.
    pub text: String,
}

#[cfg(test)]
mod tests;
