//! The dirs that efrd owns in a conversation's `$SCRATCH` (`jobs` and `attachments`):
//! made with their marker, and written only through the descriptor of the dir, never by
//! a path that a link could turn.
//!
//! The engine lets every path under `$SCRATCH` be written, and in a mode with no
//! sandbox a command can put a link or a file into such a dir, also with a name that
//! efrd uses next. So efrd opens the dir and each entry in it with `openat2` and
//! `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, makes a new file with `O_EXCL`, and moves an
//! entry that is in the way aside to `<name>.moved-<n>`, which it never opens.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{AtFlags, FileType, Mode, OFlags, ResolveFlags};
use rustix::io::Errno;

use crate::sandbox::fs::DaemonFs;

/// The resolve rule of every open below an owned dir.
const BENEATH: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS);

/// The flags of an open of a dir.
const DIR_FLAGS: OFlags =
    OFlags::RDONLY.union(OFlags::DIRECTORY).union(OFlags::CLOEXEC).union(OFlags::NOFOLLOW);

/// The most `<name>.moved-<n>` names that efrd tries for an entry that is in the way.
const ASIDE_NAMES: u32 = 1000;

/// The marker file that efrd keeps in its dir `name`, so it knows that it made the dir:
/// `.efr-jobs` in `jobs`.
pub(crate) fn marker(name: &str) -> String {
    format!(".efr-{name}")
}

/// Makes the dir `name` of `scratch` (0700) with efrd's marker file, below the
/// descriptor of `scratch`. An entry `name` without the marker (a call made it before
/// efrd did) goes aside to `<name>.moved-<n>` first. A `scratch` that does not exist yet
/// holds nothing to make. It blocks.
pub(crate) fn make(scratch: &Path, name: &str) -> io::Result<()> {
    let scratch = match DaemonFs::open(scratch, OFlags::RDONLY | OFlags::DIRECTORY) {
        Ok(fd) => fd,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let marker = marker(name);
    let marked = |dir: &OwnedFd| {
        let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        rustix::fs::openat2(dir, marker.as_str(), flags, Mode::empty(), BENEATH).is_ok()
    };
    match rustix::fs::statat(&scratch, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let is_dir = FileType::from_raw_mode(stat.st_mode) == FileType::Directory;
            let ours = is_dir
                && rustix::fs::openat2(&scratch, name, DIR_FLAGS, Mode::empty(), BENEATH)
                    .is_ok_and(|dir| marked(&dir));
            if ours {
                return Ok(());
            }
            move_aside(&scratch, name)?;
        }
        Err(Errno::NOENT) => {}
        Err(errno) => return Err(errno.into()),
    }
    match rustix::fs::mkdirat(&scratch, name, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(errno) => return Err(errno.into()),
    }
    let dir = rustix::fs::openat2(&scratch, name, DIR_FLAGS, Mode::empty(), BENEATH)?;
    let marker_flags = OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    rustix::fs::openat2(&dir, marker.as_str(), marker_flags, Mode::from_raw_mode(0o600), BENEATH)?;
    Ok(())
}

/// Opens the dir `name` of `scratch` with no link on the way. It blocks.
pub(crate) fn open(scratch: &Path, name: &str) -> io::Result<OwnedFd> {
    let scratch = DaemonFs::open(scratch, OFlags::RDONLY | OFlags::DIRECTORY)?;
    Ok(rustix::fs::openat2(&scratch, name, DIR_FLAGS, Mode::empty(), BENEATH)?)
}

/// Makes the new file `name` (0600) in the owned dir `dir` and fills it with the bytes
/// of `source` from its position on. An entry `name` that is there already, a link
/// too, goes aside first and is never opened. `std::io::copy` fills the file with
/// `copy_file_range`, so the copy shares the extents of `source` where the file system
/// can, and reads and writes after `EXDEV`. A copy that fails leaves no file. It
/// blocks.
pub(crate) fn write_new(dir: &OwnedFd, name: &str, source: &File) -> io::Result<()> {
    let flags = OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let create = || rustix::fs::openat2(dir, name, flags, Mode::from_raw_mode(0o600), BENEATH);
    let file = match create() {
        Err(Errno::EXIST) => {
            move_aside(dir, name)?;
            create()?
        }
        created => created?,
    };
    let mut file = File::from(file);
    let mut source = source;
    if let Err(error) = io::copy(&mut source, &mut file) {
        let _ = rustix::fs::unlinkat(dir, name, AtFlags::empty());
        return Err(error);
    }
    Ok(())
}

/// Removes the entry `name` of the owned dir `dir`: a link itself, never its target.
/// An entry that is gone already is no error. It blocks.
pub(crate) fn remove(dir: &OwnedFd, name: &str) -> io::Result<()> {
    match rustix::fs::unlinkat(dir, name, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(errno) => Err(errno.into()),
    }
}

/// Renames the entry `name` of `dir` to the first free `name.moved-<n>`.
fn move_aside(dir: &OwnedFd, name: &str) -> io::Result<()> {
    for n in 1..=ASIDE_NAMES {
        let aside = format!("{name}.moved-{n}");
        match rustix::fs::statat(dir, aside.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => continue,
            Err(Errno::NOENT) => {
                rustix::fs::renameat(dir, name, dir, aside.as_str())?;
                tracing::warn!(
                    name,
                    aside,
                    "an entry in scratch that efrd did not make was moved aside"
                );
                return Ok(());
            }
            Err(errno) => return Err(errno.into()),
        }
    }
    Err(io::Error::other("no free name to move an entry aside"))
}

#[cfg(test)]
mod tests;
