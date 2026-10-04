//! File operations with the guarantees that efr relies on.
//!
//! - [`write_atomic`]: a reader sees the old content or the new content, never a mix,
//!   and the new content survives a crash once the call returns.
//! - [`create_private`]: a new file that only its owner can read (mode 0600).
//! - [`claim_dir`]: a directory that exactly one of many callers creates.
//!
//! These functions block. Async code calls them inside `spawn_blocking`.

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::StdxError;

const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIR_MODE: u32 = 0o700;

/// How many temporary names [`write_atomic`] tries. A name collides only with a file
/// left behind by an earlier process that had the same pid, so a few tries are enough.
const TEMP_ATTEMPTS: u32 = 16;

/// Separates the temporary files of concurrent [`write_atomic`] calls in one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The result of [`claim_dir`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Claim {
    /// This call created the directory, so the caller owns it.
    Claimed,
    /// Something was already at the path, so another caller owns it.
    Taken,
}

/// Replaces the file at `path` with `contents`. A reader sees the old file or the new
/// one, never a mix.
///
/// The bytes go to a hidden temporary file in the same directory. That file is flushed
/// to disk and renamed over `path`, and then the directory is flushed so that the
/// rename survives a crash. The new file has mode 0600, because efr writes nothing
/// that other users may read. The parent directory must exist. A symbolic link at
/// `path` is replaced by the new file, not followed.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), StdxError> {
    let Some(file_name) = path.file_name() else {
        return Err(StdxError::NoFileName { path: path.to_path_buf() });
    };
    let dir = parent_dir(path);
    let (temp_path, mut file) = create_temp(dir, file_name)
        .map_err(|source| StdxError::CreateFile { path: path.to_path_buf(), source })?;
    if let Err(source) = file.write_all(contents).and_then(|()| file.sync_all()) {
        remove_leftover(&temp_path);
        return Err(StdxError::WriteFile { path: path.to_path_buf(), source });
    }
    drop(file);
    if let Err(source) = fs::rename(&temp_path, path) {
        remove_leftover(&temp_path);
        return Err(StdxError::Rename { from: temp_path, to: path.to_path_buf(), source });
    }
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|source| StdxError::SyncDir { path: dir.to_path_buf(), source })
}

/// Creates a new file at `path` that only its owner can read and write (mode 0600),
/// and opens it for writing.
///
/// Fails when anything is already at `path`, a symbolic link included, so it never
/// truncates an existing file or writes through a link that someone else placed.
pub fn create_private(path: &Path) -> Result<File, StdxError> {
    open_new_private(path)
        .map_err(|source| StdxError::CreateFile { path: path.to_path_buf(), source })
}

/// Creates the directory `path` with mode 0700. Only the last component is created;
/// the parent must exist.
///
/// `mkdir` is atomic, so when many callers claim one path, exactly one gets
/// [`Claim::Claimed`] and the others get [`Claim::Taken`]. Anything that is already at
/// `path`, a file included, means taken.
pub fn claim_dir(path: &Path) -> Result<Claim, StdxError> {
    match DirBuilder::new().mode(PRIVATE_DIR_MODE).create(path) {
        Ok(()) => Ok(Claim::Claimed),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => Ok(Claim::Taken),
        Err(source) => Err(StdxError::CreateDir { path: path.to_path_buf(), source }),
    }
}

/// The directory that holds `path`; a bare file name lives in the current directory.
pub(crate) fn parent_dir(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn open_new_private(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).mode(PRIVATE_FILE_MODE).open(path)
}

/// A new private file named `.<file_name>.<pid>.<n>.tmp` in `dir`. The leading dot
/// hides it from listings, and the suffix keeps it from matching a pattern such as
/// `*.json`.
fn create_temp(dir: &Path, file_name: &OsStr) -> io::Result<(PathBuf, File)> {
    let pid = std::process::id();
    let mut attempt = 0;
    loop {
        attempt += 1;
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(file_name);
        name.push(format!(".{pid}.{n}.tmp"));
        let temp_path = dir.join(name);
        match open_new_private(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(source)
                if source.kind() == io::ErrorKind::AlreadyExists && attempt < TEMP_ATTEMPTS => {}
            Err(source) => return Err(source),
        }
    }
}

/// Removes a temporary file after a failure. The failure is the error to report; a
/// hidden leftover file is harmless, so an error here is dropped.
fn remove_leftover(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests;
