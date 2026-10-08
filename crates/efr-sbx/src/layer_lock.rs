//! The lock of a conversation's cache layers in the `overlay` mode.
//!
//! The layer helper mounts each overlay with `index=off`, and without the index the
//! kernel no longer refuses a second overlay on an upper dir that one already uses.
//! Two overlays on one upper dir give undefined results. So the launcher takes an
//! exclusive `flock` on the conversation's layer dir (`$SBX/cache`) before bwrap
//! starts, and keeps it until the call's mount namespace, and with it each overlay, is
//! gone. A second launch of the conversation waits for the first one; after
//! [`LAYER_LOCK_WAIT`] it fails as a setup failure, and its call does not run.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{FlockOperation, Mode, OFlags};
use rustix::io::Errno;

use crate::error::SbxError;
use crate::{os, signals};

/// How long a launch waits for the lock of its conversation's layers.
pub(crate) const LAYER_LOCK_WAIT: Duration = Duration::from_secs(5);

/// How long the launch sleeps between two tries.
const RETRY: Duration = Duration::from_millis(5);

/// The held locks; dropping it releases them.
#[derive(Debug)]
pub(crate) struct LayerLock {
    _held: Vec<OwnedFd>,
}

/// How a try to lock went.
#[derive(Debug)]
pub(crate) enum Locked {
    /// Every dir is locked.
    Held(LayerLock),
    /// Another launch held `dir` for all of the wait.
    Busy {
        /// The dir that stayed locked.
        dir: PathBuf,
    },
}

/// The dirs to lock for the layer dirs `layer_dirs`: the dir that holds each of them,
/// once each and in order, so two launches lock them in the same order.
pub(crate) fn lock_dirs(layer_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        layer_dirs.iter().filter_map(|dir| dir.parent().map(Path::to_path_buf)).collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Locks each of `dirs` exclusively, waiting at most `wait` for each.
pub(crate) fn lock(dirs: &[PathBuf], wait: Duration) -> Result<Locked, SbxError> {
    let mut held = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fd = rustix::fs::open(dir, flags, Mode::empty())
            .map_err(|error| SbxError::io("open", dir, error.into()))?;
        let start = os::now();
        loop {
            match rustix::fs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                // NOTE: Ctrl+C ends the wait; the launcher then reports the interrupt.
                Err(Errno::WOULDBLOCK)
                    if start.elapsed() < wait && signals::interrupted().is_none() =>
                {
                    os::sleep(RETRY);
                }
                Err(Errno::WOULDBLOCK) => return Ok(Locked::Busy { dir: dir.clone() }),
                Err(Errno::INTR) => {}
                Err(error) => return Err(SbxError::io("lock", dir, error.into())),
            }
        }
        held.push(fd);
    }
    Ok(Locked::Held(LayerLock { _held: held }))
}

#[cfg(test)]
mod tests;
