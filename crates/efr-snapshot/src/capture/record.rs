//! What one snapshot left out, and why, and the modes of the files that it looked at.
//!
//! The record is for the report only: whether a snapshot holds a file is a positive
//! check against its tree (`cover.rs`), never a look at this list.

use std::fs::Metadata;
use std::os::unix::fs::PermissionsExt as _;

/// Why a snapshot left a path out.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LeftOutReason {
    /// A file above `snapshot.max_file_mib`.
    TooLarge,
    /// An ignored file above 1 MiB.
    IgnoredTooLarge,
    /// An ignored file past the 2000 that one snapshot takes.
    IgnoredCap,
    /// An ignored file in a build or package directory ([`SKIPPED_DIRS`](crate::SKIPPED_DIRS)).
    SkippedDir,
    /// An ignored file while `snapshot.ignored` is not `small`.
    IgnoredOff,
    /// A file that git could not read, such as a file that only root may read.
    Unreadable,
    /// A name that is not UTF-8.
    NotUtf8,
    /// A nested repository.
    NestedRepo,
    /// The whole root: it holds more files than `snapshot.max_files`.
    RootTooLarge,
}

/// One path that a snapshot left out.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct LeftOut {
    /// The path relative to the root; a directory ends without `/`; the empty path is
    /// the root itself.
    pub path: String,
    /// Why.
    pub reason: LeftOutReason,
}

/// The modes that need no record: what git keeps for a regular file.
const PLAIN_MODES: [u32; 2] = [0o644, 0o755];

/// What the steps of one snapshot found on the way.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Record {
    /// The paths left out.
    pub(crate) left_out: Vec<LeftOut>,
    /// Each path that the snapshot looked at with `lstat`, and its mode (`& 0o7777`)
    /// when it is a regular file whose mode git does not keep; `None` for a plain
    /// mode, a link or a path that is gone.
    pub(crate) modes: Vec<(String, Option<u32>)>,
}

impl Record {
    pub(crate) fn leave(&mut self, path: &str, reason: LeftOutReason) {
        self.left_out.push(LeftOut { path: path.trim_end_matches('/').to_owned(), reason });
    }

    /// Notes the mode of `path` from what `lstat` said (`None`: it is gone).
    pub(crate) fn look(&mut self, path: &str, metadata: Option<&Metadata>) {
        let mode = metadata
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.permissions().mode() & 0o7777)
            .filter(|mode| !PLAIN_MODES.contains(mode));
        self.modes.push((path.to_owned(), mode));
    }

    pub(crate) fn extend(&mut self, other: Record) {
        self.left_out.extend(other.left_out);
        self.modes.extend(other.modes);
    }
}
