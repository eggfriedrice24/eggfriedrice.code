//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_stdx::StdxError;

use crate::CredentialId;

/// Every way a credential operation can fail.
///
/// No variant carries secret material: a record that fails to parse is reported by
/// its id and the parser's position, never by its content.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CredentialsError {
    /// A credential id breaks the naming rules of [`CredentialId`].
    #[error("{id:?} is not a valid credential id")]
    InvalidId {
        /// The rejected id.
        id: String,
    },

    /// The secrets directory could not be created.
    #[error("could not create the secrets directory {}", .path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A file or directory could not be inspected or read.
    #[error("could not read {}", .path.display())]
    Read {
        /// The file or directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A record file could not be written.
    #[error("could not write {}", .path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// The error from the atomic write.
        #[source]
        source: StdxError,
    },

    /// A record file could not be deleted.
    #[error("could not delete {}", .path.display())]
    Delete {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A secrets file or directory can be read or written by users other than its
    /// owner. The store refuses it instead of trusting or repairing it, because the
    /// secret may already be exposed.
    #[error(
        "{} has mode {mode:04o}; other users can reach it (expected 0700 for the directory, 0600 for files)",
        .path.display()
    )]
    InsecurePermissions {
        /// The file or directory.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
    },

    /// Something other than a regular file or directory, a symbolic link included,
    /// stands where the store expects one.
    #[error("{} is not a regular {expected}", .path.display())]
    UnexpectedFileType {
        /// The path.
        path: PathBuf,
        /// What the store expected: `"file"` or `"directory"`.
        expected: &'static str,
    },

    /// A record file is larger than any record the store writes.
    #[error("{} is larger than {limit} bytes", .path.display())]
    TooLarge {
        /// The file.
        path: PathBuf,
        /// The largest size the store reads.
        limit: u64,
    },

    /// A record could not be serialised.
    #[error("could not serialise the credential {id}")]
    Encode {
        /// The credential.
        id: CredentialId,
        /// The error from the serialiser.
        #[source]
        source: serde_json::Error,
    },

    /// A stored record is not valid JSON or has a field of the wrong type.
    #[error("the stored credential {id} is malformed")]
    Decode {
        /// The credential.
        id: CredentialId,
        /// The error from the parser.
        #[source]
        source: serde_json::Error,
    },

    /// A stored record lacks a field that its kind requires.
    #[error("the stored credential {id} has no {field} field")]
    MissingField {
        /// The credential.
        id: CredentialId,
        /// The missing field.
        field: &'static str,
    },

    /// A stored record was written in a format version this build cannot read.
    #[error(
        "the stored credential {id} has format version {version}, which this build cannot read"
    )]
    UnsupportedVersion {
        /// The credential.
        id: CredentialId,
        /// The version in the record.
        version: u32,
    },

    /// The platform keyring failed. Built only with the `keyring` feature, but always
    /// declared so that the error type does not change with features.
    #[error("the keyring failed for the credential {id}")]
    Keyring {
        /// The credential.
        id: CredentialId,
        /// The error from the keyring.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

#[cfg(test)]
mod tests;
