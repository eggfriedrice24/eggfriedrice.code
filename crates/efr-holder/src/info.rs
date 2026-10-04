//! What a holder reports about the PTYs it holds.

use efr_protocol::{PtyId, Size};
use serde::{Deserialize, Serialize};

/// One PTY that a holder holds, as [`PtyHolder::list`](crate::PtyHolder::list) reports
/// it.
///
/// A PTY stays listed after its child exits, with the exit in [`status`](Self::status),
/// until the caller releases it. That is how the daemon learns an exit status after a
/// restart, when nobody was reading the master as the shell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PtyInfo {
    /// The PTY's id, from its [`SpawnSpec`](crate::SpawnSpec).
    pub pty_id: PtyId,
    /// The process id of the child the holder spawned.
    pub child_pid: u32,
    /// The terminal size the PTY has now.
    pub size: Size,
    /// Whether the child still runs.
    pub status: ChildStatus,
}

/// Whether a holder's child still runs, and how it ended.
///
/// On the wire: `{"kind": "running"}`, `{"kind": "exited", "code": 0}` or
/// `{"kind": "signaled", "signal": 9}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChildStatus {
    /// The child has not exited, or the holder has not reaped it yet.
    Running,
    /// The child exited with a code.
    Exited {
        /// The exit code.
        code: i32,
    },
    /// A signal ended the child.
    Signaled {
        /// The platform's number of the signal, which may be any signal, not only a
        /// [`Signal`](crate::Signal) the daemon can send.
        signal: i32,
    },
}

impl ChildStatus {
    /// True while the child runs.
    pub const fn is_running(self) -> bool {
        matches!(self, ChildStatus::Running)
    }

    /// The exit code, when the child exited by itself. `None` while it runs and when a
    /// signal ended it, which is how `Event::ShellExited` reports the two cases.
    pub const fn exit_code(self) -> Option<i32> {
        match self {
            ChildStatus::Exited { code } => Some(code),
            ChildStatus::Running | ChildStatus::Signaled { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
