//! What the agent changed in files: the list of a call or a turn, as efr's own
//! snapshot store compares it, and the shape of a unified diff that a client shows.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most files that a [`FileChanges`] lists; the rest are counted in
/// [`FileChanges::more`].
pub const MAX_LISTED_FILES: usize = 50;

/// The most lines of the unified diff of one `write_file` call in
/// `tool_call_completed`; a last line `... N more lines` says how many are left out.
pub const MAX_CALL_DIFF_LINES: usize = 2000;

/// The most lines of the unified diff that `conversation.diff` returns; a last line
/// `... N more lines` says how many are left out.
pub const MAX_TURN_DIFF_LINES: usize = 20_000;

/// What happened to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChangeKind {
    /// The file is new.
    Added,
    /// The file's content, mode or link target changed.
    Modified,
    /// The file is gone.
    Deleted,
    /// The file moved; [`FileChange::from`] names where it was.
    Renamed,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChange {
    /// The file as a client shows it: relative to the turn's project root, under
    /// `$SCRATCH/` for the conversation's scratch directory, under `~/` in the home
    /// directory, otherwise absolute.
    pub path: String,
    /// What happened.
    pub kind: ChangeKind,
    /// Where a renamed file was, in the same form as `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Lines added; 0 for a binary file.
    pub added: u32,
    /// Lines removed; 0 for a binary file.
    pub removed: u32,
    /// True when the file is binary, so no lines are counted. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

/// The files that a call or a turn changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChanges {
    /// The changed files, at most [`MAX_LISTED_FILES`], sorted by path.
    pub files: Vec<FileChange>,
    /// How many changed files `files` leaves out.
    #[serde(default)]
    pub more: u32,
    /// Lines added in all changed files, the ones left out too.
    #[serde(default)]
    pub added: u32,
    /// Lines removed in all changed files, the ones left out too.
    #[serde(default)]
    pub removed: u32,
}

impl FileChanges {
    /// The list of `files` (in any order) with totals, sorted by path and cut to
    /// [`MAX_LISTED_FILES`].
    pub fn from_files(mut files: Vec<FileChange>) -> Self {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let added = files.iter().fold(0_u32, |sum, file| sum.saturating_add(file.added));
        let removed = files.iter().fold(0_u32, |sum, file| sum.saturating_add(file.removed));
        let more = files.len().saturating_sub(MAX_LISTED_FILES);
        files.truncate(MAX_LISTED_FILES);
        FileChanges { files, more: u32::try_from(more).unwrap_or(u32::MAX), added, removed }
    }

    /// The number of changed files, the ones left out too.
    pub fn count(&self) -> u64 {
        self.files.len() as u64 + u64::from(self.more)
    }

    /// True when no file changed.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
}

#[cfg(test)]
mod tests;
