//! The call dir (`$CALL`) and the conversation's shell dir: checked before anything in
//! them is trusted, and written only by atomic renames.
//!
//! efrd makes `$CALL` with mode 0700 in the masked runtime root and writes `spec.json`
//! and `nonce` with mode 0600; efr-shell writes `line`. The launcher refuses a dir or a
//! file that is a link, that another user owns, or that group or others may use, and a
//! spec that names another call dir or conversation. Files are opened beneath the
//! dir's descriptor with no link on the way.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use efr_sandbox::{LINE_FILE, MAX_SPEC_BYTES, SPEC_FILE, SandboxSpec, is_normal};
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};
use rustix::io::Errno;

use crate::error::{CallDirProblem, SbxError};
use crate::real_fs::RealFs;

/// The most bytes of the line file.
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// A directory of the launcher's own: owned by this user, not a link, not open to
/// group or others.
#[derive(Debug)]
pub(crate) struct PrivateDir {
    path: PathBuf,
    fd: OwnedFd,
}

impl PrivateDir {
    /// Opens and checks `path`.
    pub(crate) fn open(path: &Path) -> Result<PrivateDir, SbxError> {
        let problem = |problem| SbxError::CallDir { path: path.to_path_buf(), problem };
        if !is_normal(path) {
            return Err(problem(CallDirProblem::NotAbsolute));
        }
        let fd = match RealFs::open(path, OFlags::RDONLY | OFlags::DIRECTORY) {
            Ok(fd) => fd,
            Err(error) if error.raw_os_error() == Some(Errno::LOOP.raw_os_error()) => {
                return Err(problem(CallDirProblem::Link));
            }
            Err(error) if error.raw_os_error() == Some(Errno::NOTDIR.raw_os_error()) => {
                // O_NOFOLLOW with O_DIRECTORY answers a final link with ENOTDIR.
                let link = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_symlink());
                return Err(problem(if link {
                    CallDirProblem::Link
                } else {
                    CallDirProblem::Kind
                }));
            }
            Err(error) => return Err(SbxError::io("open", path, error)),
        };
        check_private(&fd, FileType::Directory).map_err(problem)?;
        Ok(PrivateDir { path: path.to_path_buf(), fd })
    }

    /// The directory's path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn open_beneath(&self, name: &str, flags: OFlags, mode: Mode) -> rustix::io::Result<OwnedFd> {
        let flags = flags | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let resolve = ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS;
        rustix::fs::openat2(&self.fd, name, flags, mode, resolve)
    }

    /// The private file `name`, at most `limit` bytes; `None` when it does not exist.
    pub(crate) fn read(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>, SbxError> {
        let problem = |problem| SbxError::CallDir { path: self.path.join(name), problem };
        let fd = match self.open_beneath(name, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(Errno::LOOP) => return Err(problem(CallDirProblem::Link)),
            Err(error) => return Err(SbxError::io("open", self.path.join(name), error.into())),
        };
        check_private(&fd, FileType::RegularFile).map_err(problem)?;
        let mut bytes = Vec::new();
        let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
        File::from(fd)
            .take(cap)
            .read_to_end(&mut bytes)
            .map_err(|error| SbxError::io("read", self.path.join(name), error))?;
        if bytes.len() > limit {
            return Err(problem(CallDirProblem::File));
        }
        Ok(Some(bytes))
    }

    /// Writes `bytes` to `name` with mode 0600 through a temporary file and a rename,
    /// so a reader sees the old file or the whole new one.
    pub(crate) fn write_atomic(&self, name: &str, bytes: &[u8]) -> Result<(), SbxError> {
        let temp = format!(".{name}.tmp");
        let path = self.path.join(name);
        match rustix::fs::unlinkat(&self.fd, temp.as_str(), rustix::fs::AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(error) => return Err(SbxError::io("remove", self.path.join(&temp), error.into())),
        }
        let flags = OFlags::RDWR | OFlags::CREATE | OFlags::EXCL;
        let fd = self
            .open_beneath(&temp, flags, Mode::RUSR | Mode::WUSR)
            .map_err(|error| SbxError::io("create", self.path.join(&temp), error.into()))?;
        File::from(fd).write_all(bytes).map_err(|error| SbxError::io("write", &path, error))?;
        rustix::fs::renameat(&self.fd, temp.as_str(), &self.fd, name)
            .map_err(|error| SbxError::io("rename", &path, error.into()))
    }

    /// Creates the empty file `name` with mode 0600; an existing file stays.
    pub(crate) fn touch(&self, name: &str) -> Result<(), SbxError> {
        let flags = OFlags::RDWR | OFlags::CREATE;
        self.open_beneath(name, flags, Mode::RUSR | Mode::WUSR)
            .map(drop)
            .map_err(|error| SbxError::io("create", self.path.join(name), error.into()))
    }
}

/// Checks that `fd` is of `kind`, owned by this user and closed to group and others.
fn check_private(fd: &OwnedFd, kind: FileType) -> Result<(), CallDirProblem> {
    let stat = rustix::fs::fstat(fd).map_err(|_| CallDirProblem::File)?;
    if FileType::from_raw_mode(stat.st_mode) != kind {
        return Err(CallDirProblem::Kind);
    }
    if stat.st_uid != rustix::process::geteuid().as_raw() {
        return Err(CallDirProblem::Owner);
    }
    if stat.st_mode & 0o077 != 0 {
        return Err(CallDirProblem::Mode);
    }
    Ok(())
}

/// A checked call dir and its spec.
#[derive(Debug)]
pub(crate) struct CallDir {
    /// The call dir.
    pub(crate) dir: PrivateDir,
    /// The spec efrd wrote for it.
    pub(crate) spec: SandboxSpec,
}

impl CallDir {
    /// Opens `path`, checks it and its files, and reads the spec.
    pub(crate) fn open(path: &Path) -> Result<CallDir, SbxError> {
        let dir = PrivateDir::open(path)?;
        let problem = |problem| SbxError::CallDir { path: path.to_path_buf(), problem };
        let Some(bytes) = dir.read(SPEC_FILE, MAX_SPEC_BYTES)? else {
            return Err(problem(CallDirProblem::File));
        };
        let spec = SandboxSpec::from_json(&bytes)?;
        if spec.runtime.call_dir != path || path.parent() != Some(spec.runtime.shell_dir.as_path())
        {
            return Err(problem(CallDirProblem::Ids));
        }
        if dir.read(LINE_FILE, MAX_LINE_BYTES)?.is_none() {
            return Err(problem(CallDirProblem::File));
        }
        Ok(CallDir { dir, spec })
    }
}

#[cfg(test)]
mod tests;
