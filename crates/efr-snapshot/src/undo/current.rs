//! [`Current`]: what a path of a root is now, read with `lstat` and never through a
//! link, for the planner's compare. The applier in the daemon reads the path again
//! just before it writes it.

use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path};

use crate::object::{ObjectId, blob_id_of, link_id};

/// What a path is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Current {
    /// The path does not exist, or a parent of it is no directory.
    Missing,
    /// A regular file: its blob id and its mode (`& 0o7777`).
    File { id: ObjectId, mode: u32 },
    /// A symbolic link to `target`.
    Link { target: Vec<u8> },
    /// A directory, a special file, a parent that is a link, a file that changed while
    /// it was read, or a read that failed: no fact says what the path is.
    Unknown,
}

impl Current {
    /// The blob id and the mode that the undo trash keeps: the file's own mode with
    /// the file type bits, `0o100000 | (mode & 0o777)`, or `0o120000` for a link.
    pub(crate) fn trash(&self) -> Option<(ObjectId, u32)> {
        match self {
            Current::File { id, mode } => Some((id.clone(), 0o100_000 | (mode & 0o777))),
            Current::Link { target } => Some((link_id(target), 0o120_000)),
            Current::Missing | Current::Unknown => None,
        }
    }
}

/// What `relative` below the canonical `root` is now. No step follows a link: a parent
/// that is a link makes the path [`Current::Unknown`].
pub(crate) fn read(root: &Path, relative: &str) -> Current {
    let relative = Path::new(relative);
    let mut at = root.to_path_buf();
    let parts: Vec<Component<'_>> = relative.components().collect();
    let Some((last, parents)) = parts.split_last() else { return Current::Unknown };
    for part in parents {
        let Component::Normal(name) = part else { return Current::Unknown };
        at.push(name);
        match fs::symlink_metadata(&at) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(metadata) if metadata.file_type().is_symlink() => return Current::Unknown,
            Ok(_) => return Current::Missing,
            Err(error) if gone(&error) => return Current::Missing,
            Err(_) => return Current::Unknown,
        }
    }
    let Component::Normal(name) = last else { return Current::Unknown };
    at.push(name);
    let seen = match fs::symlink_metadata(&at) {
        Ok(seen) => seen,
        Err(error) if gone(&error) => return Current::Missing,
        Err(_) => return Current::Unknown,
    };
    if seen.file_type().is_symlink() {
        return match fs::read_link(&at) {
            Ok(target) => Current::Link { target: target.as_os_str().as_bytes().to_vec() },
            Err(_) => Current::Unknown,
        };
    }
    if !seen.is_file() {
        return Current::Unknown;
    }
    let Ok(mut file) = fs::File::open(&at) else { return Current::Unknown };
    let Ok(opened) = file.metadata() else { return Current::Unknown };
    if (opened.dev(), opened.ino()) != (seen.dev(), seen.ino()) || !opened.is_file() {
        return Current::Unknown;
    }
    match blob_id_of(opened.len(), &mut file) {
        Ok(Some(id)) => Current::File { id, mode: opened.permissions().mode() & 0o7777 },
        Ok(None) | Err(_) => Current::Unknown,
    }
}

/// The bytes of `relative` below `root` when it is still a regular file with the blob
/// `id`, or the target of a link whose blob is `id`; `None` otherwise.
pub(crate) fn bytes_of(root: &Path, relative: &str, id: &ObjectId) -> Option<Vec<u8>> {
    match read(root, relative) {
        Current::File { id: found, .. } if &found == id => {
            let bytes = fs::read(root.join(relative)).ok()?;
            (crate::object::blob_id(&bytes) == *id).then_some(bytes)
        }
        Current::Link { target } if link_id(&target) == *id => Some(target),
        _ => None,
    }
}

/// The `(dev, ino)` of `path`, without following a last link.
pub(crate) fn dev_ino(path: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(path).ok().map(|metadata| (metadata.dev(), metadata.ino()))
}

fn gone(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}
