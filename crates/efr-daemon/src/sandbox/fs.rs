//! [`DaemonFs`]: `efr_sandbox::FsView` over the real file system, as efrd reads it for
//! the sandbox: every open goes through `openat2(RESOLVE_NO_SYMLINKS)`, so a link that
//! a contained call planted is never followed by accident.
//!
//! It also makes the target of an approved write grant (`WriteBind::MakeFile` and
//! `MakeDir`) through the descriptor of its parent, with no link anywhere on the way.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use efr_sandbox::{FileKind, FsView};
use rustix::fs::{CWD, FileType, Mode, OFlags, ResolveFlags};

/// The real file system, seen without following links where it matters.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DaemonFs;

impl DaemonFs {
    /// Opens `path` with no symbolic link anywhere on the way, with `flags` plus
    /// `O_CLOEXEC | O_NOFOLLOW`.
    pub(crate) fn open(path: &Path, flags: OFlags) -> io::Result<OwnedFd> {
        let flags = flags | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        Ok(rustix::fs::openat2(CWD, path, flags, Mode::empty(), ResolveFlags::NO_SYMLINKS)?)
    }

    /// The content of the plain file `path`, at most `limit` bytes.
    pub(crate) fn read(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        DaemonFs.read_file(path, limit)
    }

    /// Makes `target` as an empty file, or an empty directory when `dir`, through its
    /// parent's descriptor, which is opened with no link on the way. A target that is
    /// there already is left as it is, so a "yes" that comes twice changes nothing.
    pub(crate) fn make(target: &Path, dir: bool) -> io::Result<()> {
        let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        let parent = DaemonFs::open(parent, OFlags::RDONLY | OFlags::DIRECTORY)?;
        let made = if dir {
            rustix::fs::mkdirat(&parent, name, Mode::from_raw_mode(0o755))
        } else {
            let flags =
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW;
            rustix::fs::openat(&parent, name, flags, Mode::from_raw_mode(0o644)).map(drop)
        };
        match made {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => Ok(()),
            Err(errno) => Err(errno.into()),
        }
    }
}

impl FsView for DaemonFs {
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
        let fd = DaemonFs::open(path, OFlags::RDONLY | OFlags::DIRECTORY)?;
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
        // NOTE: O_NONBLOCK, so a fifo planted where a file was cannot stall efrd.
        let fd = DaemonFs::open(path, OFlags::RDONLY | OFlags::NONBLOCK)?;
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
        DaemonFs::open(path, OFlags::PATH)
    }

    fn open_empty(&self) -> io::Result<OwnedFd> {
        DaemonFs::open(Path::new("/dev/null"), OFlags::RDONLY)
    }
}

/// A view in which the launcher's own files and dirs exist, for `sandbox.explain`:
/// they are made per call, at the first shell start or by the launcher, and the answer
/// must not depend on that.
pub(crate) struct WithAssets<'a> {
    pub(crate) inner: &'a dyn FsView,
    /// Files that exist.
    pub(crate) assets: Vec<PathBuf>,
    /// Directories that exist.
    pub(crate) dirs: Vec<PathBuf>,
}

impl FsView for WithAssets<'_> {
    fn lstat(&self, path: &Path) -> io::Result<FileKind> {
        if self.assets.iter().any(|asset| asset == path) {
            return Ok(FileKind::File);
        }
        match self.inner.lstat(path) {
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && self.assets.iter().chain(&self.dirs).any(|made| made.starts_with(path)) =>
            {
                Ok(FileKind::Dir)
            }
            other => other,
        }
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.read_link(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>> {
        self.inner.read_dir(path)
    }

    fn read_file(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        self.inner.read_file(path, limit)
    }

    fn open_no_symlinks(&self, path: &Path) -> io::Result<OwnedFd> {
        self.inner.open_no_symlinks(path)
    }

    fn open_empty(&self) -> io::Result<OwnedFd> {
        self.inner.open_empty()
    }
}
