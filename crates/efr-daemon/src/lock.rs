//! The single-instance lock: an exclusive `flock` on `$XDG_DATA_HOME/efr/daemon.lock`.
//!
//! The lock, not `daemon.json`, decides which daemon owns the database: the kernel
//! drops it when the process dies, so a crashed daemon never blocks the next one, and
//! two daemons can never both hold it.

use std::fs::{DirBuilder, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

use crate::DaemonError;

/// The data directory holds every conversation, so only its owner may enter it.
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// The held lock. Dropping it releases the lock.
#[derive(Debug)]
pub(crate) struct DaemonLock {
    path: PathBuf,
    _flock: Flock<File>,
}

impl DaemonLock {
    /// Takes the lock at `path` without waiting, creating the file and its directory.
    ///
    /// Fails with [`DaemonError::AlreadyRunning`] when another process holds it.
    pub(crate) fn acquire(path: &Path) -> Result<Self, DaemonError> {
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            DirBuilder::new()
                .recursive(true)
                .mode(DIR_MODE)
                .create(dir)
                .map_err(|source| DaemonError::Io { path: dir.to_path_buf(), source })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(FILE_MODE)
            .open(path)
            .map_err(|source| DaemonError::Io { path: path.to_path_buf(), source })?;
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(flock) => Ok(DaemonLock { path: path.to_path_buf(), _flock: flock }),
            Err((_, Errno::EWOULDBLOCK)) => {
                Err(DaemonError::AlreadyRunning { path: path.to_path_buf() })
            }
            Err((_, source)) => Err(DaemonError::Lock { path: path.to_path_buf(), source }),
        }
    }

    /// The lock file.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests;
