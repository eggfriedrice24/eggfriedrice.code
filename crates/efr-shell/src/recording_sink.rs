//! Where a shell's bytes and lifecycle go: traits the daemon implements.

use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use bytes::Bytes;
use efr_holder::ChildStatus;
use efr_protocol::{ConversationId, PtyId, Seq};

/// Receives every byte read from every hidden shell, in order, before the screen and
/// the run logic see it.
///
/// The daemon appends the bytes to the PTY recording (`efr-store`), which is what
/// `pty.attach(since_seq)` replays and where a command's full output stays when the
/// model sees it truncated. `start` is the byte offset of the chunk's first byte in
/// the PTY's stream, from 0 for each new PTY; chunks of one PTY arrive back to back.
///
/// The reader waits for `record` before it reads more, which is the backpressure from
/// a slow store back to the shell. An implementation handles its own errors: the
/// shell keeps running whether or not a chunk was stored.
#[async_trait]
pub trait RecordingSink: Send + Sync + fmt::Debug {
    /// Takes one chunk of a PTY's output.
    async fn record(&self, pty_id: PtyId, start: Seq, bytes: Bytes);
}

/// Hears when a hidden shell starts, reports a new directory and exits; the daemon
/// turns these into `ShellStarted`, `CwdChanged` and `ShellExited` events.
///
/// `notice` is called from the shell's own task and must not block or call back into
/// `ShellSessions`: an implementation queues the notice and returns.
pub trait ShellObserver: Send + Sync + fmt::Debug {
    /// Takes one notice.
    fn notice(&self, notice: ShellNotice);
}

/// One lifecycle notice of a hidden shell.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShellNotice {
    /// A new shell is running.
    Started {
        /// Its conversation.
        conversation: ConversationId,
        /// Its PTY.
        pty_id: PtyId,
        /// The shell's process id.
        pid: u32,
        /// The directory it started in.
        cwd: PathBuf,
    },
    /// The shell reported a working directory different from the last one (OSC 7).
    CwdChanged {
        /// Its conversation.
        conversation: ConversationId,
        /// Its PTY.
        pty_id: PtyId,
        /// The new directory.
        cwd: PathBuf,
        /// The host the report named, if any.
        host: Option<String>,
    },
    /// The shell ended. Its PTY is released and the conversation's next run starts a
    /// new shell.
    Exited {
        /// Its conversation.
        conversation: ConversationId,
        /// Its PTY.
        pty_id: PtyId,
        /// How it ended; `None` when the holder could not tell.
        status: Option<ChildStatus>,
    },
}

/// A sink and observer that drops everything, for callers that keep no recording and
/// for tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct Discard;

#[async_trait]
impl RecordingSink for Discard {
    async fn record(&self, _pty_id: PtyId, _start: Seq, _bytes: Bytes) {}
}

impl ShellObserver for Discard {
    fn notice(&self, _notice: ShellNotice) {}
}
