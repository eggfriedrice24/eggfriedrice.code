//! [`SandboxError`], the one error type of this crate.

use std::io;
use std::path::PathBuf;

/// Every way a sandbox spec, plan, record stream or result file can fail.
///
/// A plan error refuses the call: the model reads "the sandbox cannot run here" and
/// the reason, and the line never runs elsewhere. A records error drops the shell
/// state of one call and nothing else.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SandboxError {
    /// The spec was written by another version of efr.
    #[error("the sandbox spec has version {found}, and this build reads version {expected}")]
    SpecVersion {
        /// The version in the file.
        found: u32,
        /// The version this build writes and reads.
        expected: u32,
    },

    /// A path of the spec is not absolute or not in normal form.
    #[error("the sandbox spec's {field} {path:?} is not an absolute path in normal form")]
    SpecPath {
        /// The member of the spec.
        field: &'static str,
        /// The path.
        path: PathBuf,
    },

    /// The call dir or the shell dir of the spec does not name its call or conversation.
    #[error("the sandbox spec's {field} {path:?} does not name its own call or conversation")]
    SpecIds {
        /// The member of the spec.
        field: &'static str,
        /// The path.
        path: PathBuf,
    },

    /// A JSON file of the sandbox could not be read or written.
    #[error("the sandbox file {what} is not valid JSON of its kind")]
    Json {
        /// Which file: `spec`, `result`, `policy` or `probe`.
        what: &'static str,
        /// The decoder's error.
        #[source]
        source: serde_json::Error,
    },

    /// A file is larger than its limit.
    #[error("the sandbox file {what} has {len} bytes, more than the limit of {max}")]
    TooLarge {
        /// Which file or stream.
        what: &'static str,
        /// Its length.
        len: usize,
        /// The limit.
        max: usize,
    },

    /// A file system call through the `FsView` failed.
    #[error("the sandbox could not read {path:?}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        #[source]
        source: io::Error,
    },

    /// A path resolves through too many symbolic links.
    #[error("{path:?} resolves through too many symbolic links")]
    LinkLoop {
        /// The path.
        path: PathBuf,
    },

    /// A write root or a widening is `/`, the home directory or a directory above it.
    #[error(
        "{path:?} is the home directory, the root or above the home directory; it is never a write root"
    )]
    RootTooWide {
        /// The write root.
        path: PathBuf,
    },

    /// A write root or a widening lies at or inside a read mask.
    #[error("{root:?} lies inside the masked path {mask:?}")]
    RootUnderMask {
        /// The write root.
        root: PathBuf,
        /// The mask.
        mask: PathBuf,
    },

    /// A write root or a widening lies at or inside a floor.
    #[error("{root:?} lies inside the read-only path {floor:?}")]
    RootUnderFloor {
        /// The write root.
        root: PathBuf,
        /// The floor.
        floor: PathBuf,
    },

    /// A git dir to pin is a symbolic link, or is reached through one in a write root.
    #[error("the git dir {path:?} is a symbolic link or is reached through one")]
    SymlinkedPin {
        /// The path.
        path: PathBuf,
    },

    /// A floor is reached through a symbolic link that lies in a write root, which a
    /// sandboxed call could change.
    #[error("the read-only path {path:?} is reached through a symbolic link in a write root")]
    SymlinkedFloor {
        /// The floor as the spec names it.
        path: PathBuf,
        /// The link in the write root.
        link: PathBuf,
    },

    /// An entry of the hidden shell's `PATH` is relative or empty, so it names a
    /// directory that depends on the working directory.
    #[error("the shell's PATH has the relative entry {entry:?}")]
    RelativePathEntry {
        /// The entry.
        entry: String,
    },

    /// A file the launcher must bind into the sandbox is missing.
    #[error("the sandbox file {path:?} is missing")]
    MissingAsset {
        /// The path.
        path: PathBuf,
    },

    /// The target of a grant is missing or of the wrong kind.
    #[error("the grant's target {path:?} is missing or of the wrong kind")]
    GrantTarget {
        /// The path.
        path: PathBuf,
    },

    /// A grant asks to unmask an engine secret.
    #[error("{path:?} is an engine secret; no grant unmasks it")]
    UnmaskSecret {
        /// The path.
        path: PathBuf,
    },

    /// A session bus grant, but the spec names no session bus.
    #[error("a session bus grant, but the user has no session bus")]
    NoSessionBus,

    /// The records stream does not start with the header.
    #[error("the records do not start with the efr-records v1 header")]
    RecordsHeader,

    /// The records stream holds more records than the limit.
    #[error("the records hold more than {max} records")]
    TooManyRecords {
        /// The limit.
        max: usize,
    },

    /// A record is cut off or of an unknown kind.
    #[error("the record at byte {offset} is malformed")]
    RecordMalformed {
        /// Where the record starts.
        offset: usize,
    },

    /// The records stream has no `end` record.
    #[error("the records have no end record")]
    RecordsMissingEnd,

    /// A `cwd` record is not an absolute path without control characters.
    #[error("the cwd record is not an absolute path without control characters")]
    RecordCwd,
}
