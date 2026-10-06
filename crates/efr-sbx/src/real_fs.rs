//! [`RealFs`]: `efr_sandbox::FsView` over the real file system.
//!
//! Every open goes through `openat2(RESOLVE_NO_SYMLINKS)`: a bind source is an
//! `O_PATH` descriptor of the very object the plan checked, so a path that a sandboxed
//! call of another conversation swaps for a link after the check cannot change what
//! bwrap binds. A file read for the surface guard opens with `O_NONBLOCK`, so a fifo
//! planted where a config was cannot stall the launcher.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use efr_sandbox::{FileKind, FsView};
use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, ResolveFlags, StatxFlags};

/// The real file system, seen without following links where it matters.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RealFs;

impl RealFs {
    /// Opens `path` with no symbolic link anywhere on the way, with `flags` plus
    /// `O_CLOEXEC | O_NOFOLLOW`.
    pub(crate) fn open(path: &Path, flags: OFlags) -> io::Result<OwnedFd> {
        let flags = flags | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        Ok(rustix::fs::openat2(CWD, path, flags, Mode::empty(), ResolveFlags::NO_SYMLINKS)?)
    }

    /// True when `path` was made or changed at or after `since` (seconds and
    /// nanoseconds since the epoch), by its birth time where the file system keeps one
    /// and its change time otherwise. A path that cannot be read counts as changed.
    pub(crate) fn changed_since(path: &Path, since: (i64, u32)) -> bool {
        let mask = StatxFlags::BTIME | StatxFlags::CTIME;
        let Ok(stat) = rustix::fs::statx(CWD, path, AtFlags::SYMLINK_NOFOLLOW, mask) else {
            return true;
        };
        let born = StatxFlags::from_bits_truncate(stat.stx_mask).contains(StatxFlags::BTIME);
        let ctime = (stat.stx_ctime.tv_sec, stat.stx_ctime.tv_nsec);
        let btime = (stat.stx_btime.tv_sec, stat.stx_btime.tv_nsec);
        ctime >= since || (born && btime >= since)
    }
}

impl FsView for RealFs {
    fn lstat(&self, path: &Path) -> io::Result<FileKind> {
        let kind = fs::symlink_metadata(path)?.file_type();
        Ok(if kind.is_symlink() {
            FileKind::Symlink
        } else if kind.is_dir() {
            FileKind::Dir
        } else if kind.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        })
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>> {
        let fd = RealFs::open(path, OFlags::RDONLY | OFlags::DIRECTORY)?;
        let dir = rustix::fs::Dir::read_from(&fd)?;
        let mut names = Vec::new();
        for entry in dir {
            let name = entry?.file_name().to_bytes().to_vec();
            if name != b"." && name != b".." {
                names.push(std::os::unix::ffi::OsStringExt::from_vec(name));
            }
        }
        Ok(names)
    }

    fn read_file(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        let fd = RealFs::open(path, OFlags::RDONLY | OFlags::NONBLOCK)?;
        if FileType::from_raw_mode(rustix::fs::fstat(&fd)?.st_mode) != FileType::RegularFile {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let mut bytes = Vec::new();
        let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
        fs::File::from(fd).take(cap).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        Ok(bytes)
    }

    fn open_no_symlinks(&self, path: &Path) -> io::Result<OwnedFd> {
        RealFs::open(path, OFlags::PATH)
    }
}

#[cfg(test)]
mod tests;
