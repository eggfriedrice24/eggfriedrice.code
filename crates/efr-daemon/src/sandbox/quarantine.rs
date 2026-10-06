//! Moving quarantined changes back (efr's auto spec, section 5.6): after a call, the
//! launcher moved each git change that runs programs to `$SBX/quarantine/<call>/`, with
//! an index `entries.json` of `{from, to}`. When the user answers "yes, keep it",
//! efrd moves the named entries back, through descriptors opened with no link on the
//! way, and never over something that is there now.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::SurfaceChange;
use rustix::fs::{OFlags, RenameFlags};
use serde::Deserialize;

use crate::sandbox::fs::DaemonFs;

/// The quarantine's index file.
pub(crate) const INDEX: &str = "entries.json";

/// The most bytes of the index.
const MAX_INDEX: usize = 1024 * 1024;

/// One entry of the index.
#[derive(Debug, Deserialize)]
struct Entry {
    /// Where it was.
    from: PathBuf,
    /// Its name in the quarantine dir.
    to: String,
}

/// Moves each quarantined change of `changes` back from `dir`. Fails with the reason
/// for the model when one stays. It blocks.
pub(crate) fn restore(dir: &Path, changes: &[SurfaceChange]) -> Result<(), String> {
    let index = DaemonFs::read(&dir.join(INDEX), MAX_INDEX)
        .map_err(|error| format!("the quarantine of this call cannot be read: {error}"))?;
    let entries: Vec<Entry> = serde_json::from_slice(&index)
        .map_err(|error| format!("the quarantine index does not read: {error}"))?;
    let mut failed = Vec::new();
    for change in changes.iter().filter(|change| change.quarantined) {
        let Some(entry) = entries.iter().find(|entry| entry.from == change.path) else {
            failed.push(format!("{} is not in the quarantine", change.path.display()));
            continue;
        };
        if entry.to.contains('/') || entry.to.starts_with('.') {
            failed.push(format!("{} has a bad quarantine name", change.path.display()));
            continue;
        }
        if let Err(error) = move_back(&dir.join(&entry.to), &entry.from) {
            failed.push(format!("{} could not move back: {error}", change.path.display()));
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
}

/// Moves `from`, in the quarantine, to `to`, which must not exist.
fn move_back(from: &Path, to: &Path) -> io::Result<()> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidInput);
    let (from_dir, from_name) =
        (from.parent().ok_or_else(invalid)?, from.file_name().ok_or_else(invalid)?);
    let (to_dir, to_name) = (to.parent().ok_or_else(invalid)?, to.file_name().ok_or_else(invalid)?);
    let from_fd = DaemonFs::open(from_dir, OFlags::RDONLY | OFlags::DIRECTORY)?;
    let to_fd = DaemonFs::open(to_dir, OFlags::RDONLY | OFlags::DIRECTORY)?;
    match rustix::fs::renameat_with(&from_fd, from_name, &to_fd, to_name, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::XDEV) => {
            if fs::symlink_metadata(to).is_ok() {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            }
            copy_tree(from, to)?;
            if fs::symlink_metadata(from)?.is_dir() {
                fs::remove_dir_all(from)
            } else {
                fs::remove_file(from)
            }
        }
        Err(errno) => Err(errno.into()),
    }
}

/// Copies `from` to `to`, keeping links as links.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)
    } else if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())
    } else {
        fs::copy(from, to).map(drop)
    }
}

#[cfg(test)]
mod tests;
