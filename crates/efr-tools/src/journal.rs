//! The write journal: the original of every file a tool changes, recorded before the
//! change, so that `undo turn N` can exist later (structure document, section 6).

use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::{CallIds, ToolError};

/// What a file was before a tool changed it.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileSnapshot {
    /// The file's absolute path.
    pub path: PathBuf,
    /// What was there.
    pub original: Original,
}

impl FileSnapshot {
    /// A snapshot of `path` as `original`.
    pub fn new(path: impl Into<PathBuf>, original: Original) -> Self {
        FileSnapshot { path: path.into(), original }
    }
}

/// What was at a path before a write.
///
/// `Debug` shows the content's length, not the content: a file can hold secrets.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Original {
    /// Nothing; the write created the file.
    Missing,
    /// A regular file.
    File {
        /// The permission bits (`mode & 0o7777`).
        mode: u32,
        /// The owner's user id.
        uid: u32,
        /// The owner's group id.
        gid: u32,
        /// The whole content.
        content: Vec<u8>,
    },
}

impl fmt::Debug for FileSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSnapshot")
            .field("path", &self.path)
            .field("original", &self.original)
            .finish()
    }
}

impl fmt::Debug for Original {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Original::Missing => f.write_str("Missing"),
            Original::File { mode, uid, gid, content } => f
                .debug_struct("File")
                .field("mode", &format_args!("{mode:o}"))
                .field("uid", uid)
                .field("gid", gid)
                .field("content", &format_args!("<{} bytes>", content.len()))
                .finish(),
        }
    }
}

/// One journal entry: which call changed which file, and what it was before.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JournalEntry {
    /// The call that changes the file.
    pub ids: CallIds,
    /// The file before the change.
    pub snapshot: FileSnapshot,
}

impl JournalEntry {
    /// The entry for `snapshot`, taken by the call `ids`.
    pub fn new(ids: CallIds, snapshot: FileSnapshot) -> Self {
        JournalEntry { ids, snapshot }
    }
}

/// Keeps journal entries. The daemon persists them in the store; a tool records the
/// entry and waits for it before it changes the file, so a crash after the change
/// never loses the original.
#[async_trait]
pub trait WriteJournal: Send + Sync + fmt::Debug {
    /// Records one entry. An error stops the write; build it with
    /// [`ToolError::journal`].
    async fn record(&self, entry: JournalEntry) -> Result<(), ToolError>;
}

/// A journal in memory, for tests and for callers that keep no undo history.
#[derive(Debug, Default)]
pub struct MemoryJournal {
    entries: Mutex<Vec<JournalEntry>>,
}

impl MemoryJournal {
    /// An empty journal.
    pub fn new() -> Self {
        MemoryJournal::default()
    }

    /// Every entry so far, oldest first.
    pub fn entries(&self) -> Vec<JournalEntry> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl WriteJournal for MemoryJournal {
    async fn record(&self, entry: JournalEntry) -> Result<(), ToolError> {
        // A push cannot leave the list half-changed, so a poisoned lock still guards a
        // whole list.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).push(entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
