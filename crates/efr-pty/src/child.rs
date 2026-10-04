//! The holder's side of one child process: who reaps it, where its end is recorded and
//! how it receives signals.
//!
//! A reaper task owns the tokio child and waits for it, so every child is reaped as
//! soon as it exits, whether or not anyone asks. It records the end in a `watch`
//! channel that `list` reads and `wait` subscribes to; the channel starts at
//! [`ChildStatus::Running`] and changes exactly once, so a caller that saw a terminal
//! status from `wait` never sees `Running` from `list` afterwards. The holder's table
//! holds the only strong reference to the sender: releasing a PTY drops it, which ends
//! every pending `wait` with "not found" while the task still reaps the child.

use std::os::fd::OwnedFd;
use std::os::unix::process::ExitStatusExt as _;
use std::process::ExitStatus;
use std::sync::{Arc, Weak};

use efr_holder::{ChildStatus, Signal};
use rustix::process::{Pid, PidfdFlags, kill_process, pidfd_open, pidfd_send_signal};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::PtyError;

/// The exit code recorded when a child is gone but waiting for it failed, so how it
/// ended is unknown. Real exit codes are 0 to 255, so it cannot be mistaken for one.
pub(crate) const UNKNOWN_EXIT_CODE: i32 = -1;

/// One child process of the holder.
#[derive(Debug)]
pub(crate) struct Child {
    pid: Pid,
    /// Signals go through the pidfd, which names this process and no later one that
    /// reuses its number. `None` only on a kernel without `pidfd_open` (before 5.3).
    pidfd: Option<OwnedFd>,
    status: Arc<watch::Sender<ChildStatus>>,
}

impl Child {
    /// Takes over a child that `spawn` has just returned: opens its pidfd, then starts
    /// the reaper task on `runtime`.
    pub(crate) fn adopt(child: tokio::process::Child, runtime: &Handle) -> Result<Child, PtyError> {
        // Dropping the tokio child here hands it to tokio's orphan queue, which reaps it.
        let pid = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw)
            .ok_or(PtyError::MissingChildPid)?;
        // NOTE: the pidfd must be opened before the reaper task first polls the child.
        // Until then nothing can reap it, so the number still names this process.
        let pidfd = pidfd_open(pid, PidfdFlags::empty()).ok();
        let status = Arc::new(watch::Sender::new(ChildStatus::Running));
        runtime.spawn(reap(child, pid, Arc::downgrade(&status)));
        Ok(Child { pid, pidfd, status })
    }

    /// The process id, as the holder contract reports it.
    pub(crate) fn id(&self) -> u32 {
        // A Pid is positive, so the conversion is exact.
        self.pid.as_raw_pid().unsigned_abs()
    }

    /// How the child stands now: `Running` until the reaper task has reaped it.
    pub(crate) fn status(&self) -> ChildStatus {
        *self.status.borrow()
    }

    /// A receiver that sees the status change once the child is reaped.
    pub(crate) fn subscribe(&self) -> watch::Receiver<ChildStatus> {
        self.status.subscribe()
    }

    /// Sends `signal` to the child. Fails with `ESRCH` once the child has been reaped.
    pub(crate) fn signal(&self, signal: rustix::process::Signal) -> rustix::io::Result<()> {
        match &self.pidfd {
            Some(pidfd) => pidfd_send_signal(pidfd, signal),
            // NOTE: without a pidfd the number can name another process once the child
            // is reaped. The caller checks the status first, which leaves only the
            // moment between the reap and the status update.
            None => kill_process(self.pid, signal),
        }
    }
}

/// Waits for the child and records how it ended, if the PTY is still held.
async fn reap(
    mut child: tokio::process::Child,
    pid: Pid,
    status: Weak<watch::Sender<ChildStatus>>,
) {
    let ended = match child.wait().await {
        Ok(exit) => status_of(exit),
        Err(error) => {
            // This task is where the error is dropped, so it is logged here. waitpid
            // fails like this when something else reaped the child first.
            tracing::warn!(
                pid = pid.as_raw_pid(),
                %error,
                "could not wait for a PTY child; its exit status is unknown"
            );
            ChildStatus::Exited { code: UNKNOWN_EXIT_CODE }
        }
    };
    if let Some(sender) = status.upgrade() {
        sender.send_replace(ended);
    }
}

/// The holder contract's form of how a reaped child ended.
pub(crate) fn status_of(exit: ExitStatus) -> ChildStatus {
    if let Some(code) = exit.code() {
        ChildStatus::Exited { code }
    } else if let Some(signal) = exit.signal() {
        ChildStatus::Signaled { signal }
    } else {
        // `wait` reports neither stops nor continues, so this is not reached in
        // practice; it still must not be reported as running.
        ChildStatus::Exited { code: UNKNOWN_EXIT_CODE }
    }
}

/// The platform's signal for a contract [`Signal`], or `None` for a signal this build
/// does not know.
pub(crate) fn signal_number(signal: Signal) -> Option<rustix::process::Signal> {
    match signal {
        Signal::Hangup => Some(rustix::process::Signal::HUP),
        Signal::Interrupt => Some(rustix::process::Signal::INT),
        Signal::Quit => Some(rustix::process::Signal::QUIT),
        Signal::Terminate => Some(rustix::process::Signal::TERM),
        Signal::Kill => Some(rustix::process::Signal::KILL),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
