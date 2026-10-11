//! [`DaemonFs`]: `efr_sandbox::FsView` over the real file system, as efrd reads it for
//! the sandbox: every open goes through `openat2(RESOLVE_NO_SYMLINKS)`, so a link that
//! a contained call planted is never followed by accident.
//!
//! It also makes the target of an approved write grant (`WriteBind::MakeFile` and
//! `MakeDir`) through the descriptor of its parent, with no link anywhere on the way,
//! and deletes an allowed cache delete below the descriptor of its cache.
//!
//! The writes of `efr undo` go through [`Beneath`]: a directory opened with no link on
//! its path, and each path below it opened with `RESOLVE_BENEATH |
//! RESOLVE_NO_SYMLINKS`, so a link that a call planted can never lead a write out of a
//! root or into another file. A file is written to a new name in its directory and
//! renamed over the old one, so a reader never sees half of it.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};

use efr_sandbox::{FileKind, FsView};
use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, ResolveFlags};

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

    /// Deletes `relative`, a file or a whole directory, below the directory `root`, and
    /// returns the bytes that it freed on the disk; `None` when nothing is there. The
    /// root opens with no link anywhere on its path, and every path below it with
    /// `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, so a link can never lead the delete out
    /// of `root`. A directory that its owner made read-only, as Go's module cache does,
    /// is opened up first. It blocks.
    pub(crate) fn remove_beneath(root: &Path, relative: &Path) -> io::Result<Option<u64>> {
        let (Some(parent), Some(name)) = (relative.parent(), relative.file_name()) else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        if relative.is_absolute()
            || relative.components().any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let root = DaemonFs::open(root, OFlags::RDONLY | OFlags::DIRECTORY)?;
        let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let resolve = ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS;
        let parent = match rustix::fs::openat2(&root, parent, flags, Mode::empty(), resolve) {
            Ok(parent) => parent,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(errno) => return Err(errno.into()),
        };
        remove_entry(&parent, name, 0)
    }
}

/// What one entry of a directory is, read without following a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Node {
    /// Nothing is there.
    Missing,
    /// A regular file, its bytes and its mode (`& 0o7777`).
    File { bytes: Vec<u8>, mode: u32 },
    /// A symbolic link and its target.
    Link(Vec<u8>),
    /// A directory or a special file.
    Other,
}

/// A directory opened with no link anywhere on its path. Every path below it opens
/// with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, so nothing below it can lead out.
#[derive(Debug)]
pub(crate) struct Beneath {
    fd: OwnedFd,
}

/// How many names [`Beneath::replace`] tries for its new file before it gives up.
const TEMP_TRIES: u32 = 16;

impl Beneath {
    /// Opens the directory `dir`, with no link anywhere on its path.
    pub(crate) fn open(dir: &Path) -> io::Result<Beneath> {
        Ok(Beneath { fd: DaemonFs::open(dir, OFlags::RDONLY | OFlags::DIRECTORY)? })
    }

    /// The parent directory of `relative` below this one, and the last name of
    /// `relative`. With `make`, each missing directory on the way is made with `mode`.
    /// `None` when a directory on the way is missing and `make` is false. A path that
    /// is not plain (absolute, `..`, `.`) or a link or a file on the way is an error.
    pub(crate) fn parent(
        &self,
        relative: &Path,
        make: Option<u32>,
    ) -> io::Result<Option<(Beneath, OsString)>> {
        let parts: Vec<&OsStr> = relative
            .components()
            .map(|part| match part {
                Component::Normal(name) => Ok(name),
                _ => Err(io::Error::from(io::ErrorKind::InvalidInput)),
            })
            .collect::<io::Result<_>>()?;
        let Some((name, dirs)) = parts.split_last() else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        let mut at = Beneath { fd: rustix::io::dup(&self.fd)? };
        for dir in dirs {
            at = match at.child_dir(dir) {
                Ok(child) => child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => match make {
                    Some(mode) => {
                        match rustix::fs::mkdirat(&at.fd, *dir, Mode::from_raw_mode(mode)) {
                            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                            Err(errno) => return Err(errno.into()),
                        }
                        at.child_dir(dir)?
                    }
                    None => return Ok(None),
                },
                Err(error) => return Err(error),
            };
        }
        Ok(Some((at, name.to_os_string())))
    }

    /// The directory `name` of this one, never through a link.
    fn child_dir(&self, name: &OsStr) -> io::Result<Beneath> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let resolve = ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS;
        let fd = rustix::fs::openat2(&self.fd, name, flags, Mode::empty(), resolve)?;
        Ok(Beneath { fd })
    }

    /// The `(dev, ino)` of this directory.
    pub(crate) fn dev_ino(&self) -> io::Result<(u64, u64)> {
        let stat = rustix::fs::fstat(&self.fd)?;
        Ok((stat.st_dev, stat.st_ino))
    }

    /// What the entry `name` is, with the bytes of a file of at most `limit` bytes. A
    /// larger file is an error.
    pub(crate) fn node(&self, name: &OsStr, limit: usize) -> io::Result<Node> {
        let stat = match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(Node::Missing),
            Err(errno) => return Err(errno.into()),
        };
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Symlink => {
                let target = rustix::fs::readlinkat(&self.fd, name, Vec::new())?;
                Ok(Node::Link(target.into_bytes()))
            }
            FileType::RegularFile => {
                // NOTE: O_NONBLOCK, so a fifo planted after the stat cannot stall efrd.
                let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
                let fd = rustix::fs::openat(&self.fd, name, flags, Mode::empty())?;
                let stat = rustix::fs::fstat(&fd)?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                    return Ok(Node::Other);
                }
                let mut bytes = Vec::new();
                let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
                fs::File::from(fd).take(cap).read_to_end(&mut bytes)?;
                if bytes.len() > limit {
                    return Err(io::Error::from(io::ErrorKind::FileTooLarge));
                }
                Ok(Node::File { bytes, mode: stat.st_mode & 0o7777 })
            }
            _ => Ok(Node::Other),
        }
    }

    /// Writes `bytes` as the file `name` with `mode` (`& 0o777`): to a new name in this
    /// directory first, then renamed over whatever `name` is, a link included.
    pub(crate) fn replace(&self, name: &OsStr, bytes: &[u8], mode: u32) -> io::Result<()> {
        let mode = Mode::from_raw_mode(mode & 0o777);
        let flags =
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let (temp, fd) = self.temp(name, |temp| rustix::fs::openat(&self.fd, temp, flags, mode))?;
        // NOTE: the umask may have taken bits of the mode that the file had.
        let written = rustix::fs::fchmod(&fd, mode)
            .map_err(io::Error::from)
            .and_then(|()| {
                let mut file = fs::File::from(fd);
                file.write_all(bytes)?;
                file.sync_all()
            })
            .and_then(|()| Ok(rustix::fs::renameat(&self.fd, &temp, &self.fd, name)?));
        if written.is_err() {
            let _ = rustix::fs::unlinkat(&self.fd, &temp, AtFlags::empty());
        }
        written
    }

    /// Makes `name` a symbolic link to `target`: a new link first, then renamed over
    /// whatever `name` is.
    pub(crate) fn link(&self, name: &OsStr, target: &[u8]) -> io::Result<()> {
        let target = OsStr::from_bytes(target);
        let (temp, ()) = self.temp(name, |temp| rustix::fs::symlinkat(target, &self.fd, temp))?;
        let renamed = rustix::fs::renameat(&self.fd, &temp, &self.fd, name);
        if renamed.is_err() {
            let _ = rustix::fs::unlinkat(&self.fd, &temp, AtFlags::empty());
        }
        Ok(renamed?)
    }

    /// Writes `bytes` as the new file `name` with `mode`, or as a link when `link`;
    /// an entry that is there already gets the next free name `name.1`, `name.2`, and
    /// so on. Returns the name that it wrote.
    pub(crate) fn add(
        &self,
        name: &OsStr,
        bytes: &[u8],
        mode: u32,
        link: bool,
    ) -> io::Result<OsString> {
        for n in 0..TEMP_TRIES {
            let mut candidate = name.to_os_string();
            if n > 0 {
                candidate.push(format!(".{n}"));
            }
            let made = if link {
                rustix::fs::symlinkat(OsStr::from_bytes(bytes), &self.fd, &candidate).map(|()| None)
            } else {
                let flags = OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::EXCL
                    | OFlags::CLOEXEC
                    | OFlags::NOFOLLOW;
                rustix::fs::openat(&self.fd, &candidate, flags, Mode::from_raw_mode(mode & 0o777))
                    .map(Some)
            };
            match made {
                Ok(None) => return Ok(candidate),
                Ok(Some(fd)) => {
                    rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode & 0o777))?;
                    let mut file = fs::File::from(fd);
                    file.write_all(bytes)?;
                    file.sync_all()?;
                    return Ok(candidate);
                }
                Err(rustix::io::Errno::EXIST) => {}
                Err(errno) => return Err(errno.into()),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    /// Removes the file or link `name`; a missing one is no error.
    pub(crate) fn remove(&self, name: &OsStr) -> io::Result<()> {
        match rustix::fs::unlinkat(&self.fd, name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(errno) => Err(errno.into()),
        }
    }

    /// Runs `make` with a new name next to `name` until one is free.
    fn temp<T>(
        &self,
        name: &OsStr,
        make: impl Fn(&OsStr) -> rustix::io::Result<T>,
    ) -> io::Result<(OsString, T)> {
        let pid = std::process::id();
        for n in 0..TEMP_TRIES {
            let mut temp = OsString::from(format!(".efr-undo-{pid}-{n}-"));
            temp.push(name);
            match make(&temp) {
                Ok(made) => return Ok((temp, made)),
                Err(rustix::io::Errno::EXIST) => {}
                Err(errno) => return Err(errno.into()),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }
}

/// How deep [`DaemonFs::remove_beneath`] goes below the path it deletes: one open
/// descriptor per level.
const MAX_REMOVE_DEPTH: usize = 256;

/// Deletes the entry `name` of `dir`, a directory with everything below it, and returns
/// the bytes that it freed; `None` when it is not there.
fn remove_entry(dir: &OwnedFd, name: &OsStr, depth: usize) -> io::Result<Option<u64>> {
    let stat = match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(errno) => return Err(errno.into()),
    };
    let blocks = u64::try_from(stat.st_blocks).unwrap_or(0).saturating_mul(512);
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        rustix::fs::unlinkat(dir, name, AtFlags::empty())?;
        // NOTE: a file with another hard link frees nothing yet.
        return Ok(Some(if stat.st_nlink == 1 { blocks } else { 0 }));
    }
    if depth >= MAX_REMOVE_DEPTH {
        return Err(io::Error::other("the directory is too deep to delete"));
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let sub = match rustix::fs::openat(dir, name, flags, Mode::empty()) {
        Ok(sub) => sub,
        Err(rustix::io::Errno::ACCESS) => {
            rustix::fs::chmodat(dir, name, Mode::from_raw_mode(0o700), AtFlags::empty())?;
            rustix::fs::openat(dir, name, flags, Mode::empty())?
        }
        Err(errno) => return Err(errno.into()),
    };
    // NOTE: the entries of a read-only directory cannot go.
    rustix::fs::fchmod(&sub, Mode::from_raw_mode(0o700))?;
    let mut names: Vec<OsString> = Vec::new();
    for entry in rustix::fs::Dir::read_from(&sub)? {
        let entry = entry?;
        let child = entry.file_name().to_bytes();
        if child != b"." && child != b".." {
            names.push(std::os::unix::ffi::OsStringExt::from_vec(child.to_vec()));
        }
    }
    let mut freed = blocks;
    for child in names {
        freed = freed.saturating_add(remove_entry(&sub, &child, depth + 1)?.unwrap_or(0));
    }
    rustix::fs::unlinkat(dir, name, AtFlags::REMOVEDIR)?;
    Ok(Some(freed))
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
