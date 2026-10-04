//! The holder boundary: the trait and the handle it returns.

use std::fmt;
use std::os::fd::OwnedFd;

use async_trait::async_trait;
use efr_protocol::{PtyId, Size};

use crate::{HolderError, PtyInfo, Signal, SignalTarget, SpawnSpec};

/// Whoever opens PTYs and keeps their child processes.
///
/// At milestone 1 that is `efr_pty::LocalPtyHolder`, in the daemon's own process; at
/// milestone 5 it is `efr-ptyd`, reached over the holder socket, which keeps the shells
/// alive while the daemon restarts. `efr-shell` holds an `Arc<dyn PtyHolder>` and never
/// learns which one it has, which is why the trait is dyn-compatible (through
/// `async-trait`) and asks for `Send + Sync`.
///
/// The holder owns the child: it reaps it and remembers how it ended. The caller owns
/// the master descriptor in the [`PtyHandle`]: it reads output from it, writes input to
/// it and closes it by dropping the handle. Every method takes the PTY by id, so a
/// holder answers the same way whether or not the caller still has the descriptor.
#[async_trait]
pub trait PtyHolder: Send + Sync + fmt::Debug {
    /// Opens a PTY of `spec.size` and starts `spec.program` on it, as a session leader
    /// with the PTY as its controlling terminal, in `spec.cwd`, with exactly `spec.env`.
    ///
    /// Fails with a spec variant of [`HolderError`] when [`SpawnSpec::validate`] does,
    /// before anything is opened, and with [`HolderError::AlreadyExists`] when the holder
    /// already holds a PTY with `spec.pty_id`.
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError>;

    /// Sets the terminal size of a PTY. The kernel then sends `SIGWINCH` to the PTY's
    /// foreground process group. A PTY whose child has exited can still be resized.
    async fn resize(&self, pty_id: PtyId, size: Size) -> Result<(), HolderError>;

    /// Delivers `signal` to `target`. Fails with [`HolderError::Exited`] when the child
    /// has already exited.
    async fn signal(
        &self,
        pty_id: PtyId,
        signal: Signal,
        target: SignalTarget,
    ) -> Result<(), HolderError>;

    /// Every PTY the holder holds, in no particular order, including PTYs whose child
    /// has exited but which nobody has released.
    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError>;

    /// Forgets a PTY. The holder closes its own copy of the master, if it keeps one, so
    /// the child gets `SIGHUP` once the caller's copy is closed too; the holder still
    /// reaps the child. Afterwards every method fails for `pty_id` with
    /// [`HolderError::NotFound`].
    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError>;
}

/// A new PTY with a child process on it, as [`PtyHolder::spawn`] returns it.
///
/// The fields are public so that a holder can build the handle and the caller can take
/// the master out of it. The master is an [`OwnedFd`] from the start, so no raw
/// descriptor number travels between crates and the descriptor is closed exactly once.
#[derive(Debug)]
pub struct PtyHandle {
    /// The master side of the PTY: the child's output is read from it and its input is
    /// written to it.
    pub master: OwnedFd,
    /// The process id of the child.
    pub child_pid: u32,
    /// The PTY's id, from the spec.
    pub pty_id: PtyId,
}

#[cfg(test)]
mod tests;
