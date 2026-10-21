//! A running call that a background job adopted: the user pressed Ctrl+B in the turn
//! view, and the command goes on in its old shell, which the job owns from then on.
//!
//! The run of the call answers its caller with [`Completion::Moved`](crate::Completion)
//! and the output so far, and the conversation's slot lets go of the shell, so the next
//! run starts a new shell in the old one's directory. The old shell's actor keeps the
//! command as its adopted run:
//!
//! - Its end answers [`AdoptedRun::ended`] with the exit status (the `D` mark) or the
//!   launcher's result (the facts of a sandboxed call), as a run's end does.
//! - Each chunk of its output goes to a bounded queue with `try_send`, so the actor
//!   never waits for the job; a full queue drops the chunk, and the reader writes how
//!   many bytes it lost. [`TeeReader`] reads the queue as plain text through the
//!   streaming cleaner.
//! - A task looks for input once per quiet period, as the watch of a command left
//!   running does, and tells each change on [`AdoptedRun::looks`]. Nobody follows a
//!   job, so a hidden wait or a visible wait that looks secret stops the command at
//!   once: `SIGINT` to the job that waits, while it holds the terminal. The job reads
//!   the wait on `looks` and closes the shell when the command did not end: it kills
//!   what is below the shell first, which efr-shell cannot do.
//!
//! The old shell is out of the conversation's slot, so no state, write, answer,
//! interrupt or close of the conversation reaches it. Its end is a
//! [`ShellNotice::AdoptedExited`](crate::ShellNotice::AdoptedExited), never an
//! `Exited`.

use std::fmt;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use efr_holder::{HolderError, PtyHolder, Signal, SignalTarget};
use efr_protocol::{InputWait, PtyId, Seq};
use efr_sandbox::SandboxResult;
use efr_stdx::time::Clock;
use tokio::sync::{oneshot, watch};

use crate::ShellError;
use crate::capture::{Kept, StreamClean};
use crate::input::{InputWatch, Offer, Quiet};
use crate::session::{Looked, RunEnd, SessionHandle};

/// How many chunks of an adopted run's output wait for the job's reader at most. A
/// chunk holds at most one read of the PTY (16 KiB).
pub(crate) const TEE_CAPACITY: usize = 256;

/// A running call that a background job adopted, as
/// [`ShellSessions::request_move`](crate::ShellSessions::request_move) hands it over.
///
/// The job owns the old shell and its PTY from now on: it reads the output
/// ([`take_output`](Self::take_output)), waits for the end ([`ended`](Self::ended)),
/// stops the command ([`interrupt`](Self::interrupt), [`close`](Self::close)) and closes
/// the shell once the command ended. Dropping it closes nothing.
pub struct AdoptedRun {
    session: SessionHandle,
    call_dir: Option<PathBuf>,
    head: String,
    output: Option<TeeReader>,
    end: Option<oneshot::Receiver<Result<RunEnd, ShellError>>>,
    looks: watch::Receiver<(InputWait, bool)>,
    holder: Arc<dyn PtyHolder>,
    clock: Arc<dyn Clock>,
    close_grace: Duration,
}

/// How an adopted run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedEnd {
    /// The exit status that the command's `D` mark carried, when it carried one.
    pub exit_code: Option<i32>,
    /// The launcher's `result.json`, for a sandboxed call that ended with one.
    pub sandbox: Option<SandboxResult>,
}

/// The parts of an adopted run that the manager hands over.
pub(crate) struct Parts {
    pub(crate) session: SessionHandle,
    pub(crate) call_dir: Option<PathBuf>,
    pub(crate) head: String,
    pub(crate) output: TeeReader,
    pub(crate) end: oneshot::Receiver<Result<RunEnd, ShellError>>,
    pub(crate) looks: watch::Receiver<(InputWait, bool)>,
    pub(crate) holder: Arc<dyn PtyHolder>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) close_grace: Duration,
}

impl AdoptedRun {
    pub(crate) fn new(parts: Parts) -> Self {
        AdoptedRun {
            session: parts.session,
            call_dir: parts.call_dir,
            head: parts.head,
            output: Some(parts.output),
            end: Some(parts.end),
            looks: parts.looks,
            holder: parts.holder,
            clock: parts.clock,
            close_grace: parts.close_grace,
        }
    }

    /// The old shell's PTY.
    pub fn pty_id(&self) -> PtyId {
        self.session.pty_id
    }

    /// The old shell's process id. Every process that the command starts stays below
    /// it: the shell is a child subreaper.
    pub fn shell_pid(&self) -> u32 {
        self.session.pid
    }

    /// The call's dir `$R/sbx/<conversation>/<call>`, for a call through the sandbox's
    /// launcher; `None` for a call that runs in the shell.
    pub fn call_dir(&self) -> Option<&Path> {
        self.call_dir.as_deref()
    }

    /// The output up to the move, as the call's result has it: within the run's output
    /// limit, with a `[... N bytes omitted ...]` line where bytes were dropped.
    pub fn head(&self) -> &str {
        &self.head
    }

    /// The output after the move as plain text, for the job's output file; `None` once
    /// it was taken. It ends when the command does.
    pub fn take_output(&mut self) -> Option<TeeReader> {
        self.output.take()
    }

    /// Waits until the command ends. It fails with [`ShellError::Exited`] when the
    /// shell ended first (a close), and once the end was told already. Dropping the
    /// future loses nothing.
    pub async fn ended(&mut self) -> Result<AdoptedEnd, ShellError> {
        let Some(end) = self.end.as_mut() else {
            return Err(self.session.exited());
        };
        let ended = end.await;
        self.end = None;
        let RunEnd { output, .. } = ended.map_err(|_| self.session.exited())??;
        Ok(AdoptedEnd {
            exit_code: output.exit_code,
            sandbox: output.sandbox.map(|result| *result),
        })
    }

    /// Sends `SIGINT` to the job in the terminal's foreground, as Ctrl+C at the terminal
    /// would; a launcher then writes its result. Nothing is sent while the shell itself
    /// holds the terminal: the command ended, and the signal could cut short the hook
    /// that prints its `D`.
    pub async fn interrupt(&self) -> Result<(), ShellError> {
        let conversation = self.session.conversation;
        match self.session.terminal.holds(self.session.pid) {
            Ok(false) => {}
            Ok(true) => return Ok(()),
            Err(source) => return Err(ShellError::Terminal { conversation, source }),
        }
        match self
            .holder
            .signal(self.session.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)
            .await
        {
            Ok(()) | Err(HolderError::Exited { .. } | HolderError::NotFound { .. }) => Ok(()),
            Err(source) => Err(ShellError::Holder { conversation, source }),
        }
    }

    /// Closes the old shell: `SIGHUP`, then `SIGKILL` when it has not ended within the
    /// close grace. The command ends with it, unless it left the shell's session.
    pub async fn close(&self) -> Result<(), ShellError> {
        self.session.close(&*self.holder, &*self.clock, self.close_grace).await
    }

    /// The command's input waits after the move, one value per change: the wait and
    /// whether a visible wait looks like a prompt for a secret. A hidden wait or a
    /// visible wait that looks secret gets `SIGINT` at once, because nobody follows a
    /// job; the job closes the shell when the command does not end. A plain visible
    /// wait stays.
    pub fn looks(&self) -> watch::Receiver<(InputWait, bool)> {
        self.looks.clone()
    }
}

impl fmt::Debug for AdoptedRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdoptedRun")
            .field("conversation", &self.session.conversation)
            .field("pty_id", &self.session.pty_id)
            .field("shell_pid", &self.session.pid)
            .field("call_dir", &self.call_dir)
            .field("head_bytes", &self.head.len())
            .finish_non_exhaustive()
    }
}

/// One chunk of an adopted run's output for its reader. An empty chunk stands for a
/// shell mark that the session took out of the stream.
#[derive(Debug)]
pub(crate) struct TeeChunk {
    /// How many bytes the queue dropped right before this chunk.
    dropped_before: u64,
    bytes: Bytes,
}

/// The session actor's side of an adopted run's output: it never waits.
#[derive(Debug)]
pub(crate) struct Tee {
    sender: SyncSender<TeeChunk>,
    /// Bytes dropped since the last chunk that went out.
    dropped: u64,
    /// Where the bytes dropped after the last chunk go when the output ends.
    lost: Arc<AtomicU64>,
}

impl Tee {
    /// Hands `bytes` to the reader, or counts them as dropped when its queue is full.
    pub(crate) fn send(&mut self, bytes: Bytes) {
        let len = bytes.len() as u64;
        match self.sender.try_send(TeeChunk { dropped_before: self.dropped, bytes }) {
            Ok(()) => self.dropped = 0,
            Err(TrySendError::Full(_)) => self.dropped = self.dropped.saturating_add(len),
            // The job stopped reading; nobody wants the output.
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Tells the reader that a shell mark stood here, so the start of an escape
    /// sequence before it is no part of what follows.
    pub(crate) fn mark(&mut self) {
        let mark = TeeChunk { dropped_before: self.dropped, bytes: Bytes::new() };
        if self.sender.try_send(mark).is_ok() {
            self.dropped = 0;
        }
    }
}

impl Drop for Tee {
    fn drop(&mut self) {
        self.lost.fetch_add(self.dropped, Ordering::Release);
    }
}

/// The output of an adopted run after the move, as plain text: escape sequences
/// dropped, a line rewritten with `\r` given out in its last state (see
/// [`ShellSessions::request_move`](crate::ShellSessions::request_move)). Where the
/// queue dropped bytes because the job read too slowly, a line `[efr: N bytes of output
/// dropped]` stands. It ends when the command ends or the shell does.
///
/// [`read`](io::Read::read) blocks until output comes, so it belongs on a thread of its
/// own.
pub struct TeeReader {
    /// In a lock only so that the reader, and the adopted run that holds it, can be
    /// shared between threads; one thread reads.
    chunks: Mutex<Receiver<TeeChunk>>,
    cleaner: StreamClean,
    lost: Arc<AtomicU64>,
    /// Text to give out before the next chunk is read.
    pending: Vec<u8>,
    done: bool,
}

impl fmt::Debug for TeeReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TeeReader")
            .field("pending", &self.pending.len())
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// A new output queue: the actor's end and the reader's.
pub(crate) fn tee() -> (Tee, TeeReader) {
    let (sender, chunks) = sync_channel(TEE_CAPACITY);
    let lost = Arc::new(AtomicU64::new(0));
    let tee = Tee { sender, dropped: 0, lost: Arc::clone(&lost) };
    let reader = TeeReader {
        chunks: Mutex::new(chunks),
        cleaner: StreamClean::default(),
        lost,
        pending: Vec::new(),
        done: false,
    };
    (tee, reader)
}

/// The line that stands for `bytes` that the queue dropped.
fn dropped_line(bytes: u64) -> String {
    format!("[efr: {bytes} bytes of output dropped]\n")
}

impl TeeReader {
    /// Reads the next chunk into `pending`; false once the output ended and all of it
    /// was given out.
    fn fill(&mut self) -> bool {
        if self.done {
            return false;
        }
        let next = self.chunks.get_mut().unwrap_or_else(PoisonError::into_inner).recv();
        match next {
            Ok(TeeChunk { dropped_before, bytes }) => {
                if dropped_before > 0 {
                    let line = self.cleaner.break_line();
                    self.pending.extend_from_slice(line.as_bytes());
                    self.pending.extend_from_slice(dropped_line(dropped_before).as_bytes());
                }
                if bytes.is_empty() {
                    self.cleaner.mark();
                } else {
                    let text = self.cleaner.push(&bytes);
                    self.pending.extend_from_slice(text.as_bytes());
                }
            }
            Err(_) => {
                self.done = true;
                let rest = self.cleaner.finish();
                self.pending.extend_from_slice(rest.as_bytes());
                let lost = self.lost.swap(0, Ordering::Acquire);
                if lost > 0 {
                    if self.pending.last().is_some_and(|byte| *byte != b'\n') {
                        self.pending.push(b'\n');
                    }
                    self.pending.extend_from_slice(dropped_line(lost).as_bytes());
                }
            }
        }
        true
    }
}

impl io::Read for TeeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.pending.is_empty() {
            if !self.fill() {
                return Ok(0);
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

/// The task that looks for input of an adopted run until the command ends.
pub(crate) struct AdoptedWatch {
    pub(crate) session: SessionHandle,
    pub(crate) id: u64,
    pub(crate) offer: Offer,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) holder: Arc<dyn PtyHolder>,
    pub(crate) quiet: Quiet,
    pub(crate) quiet_period: Duration,
    pub(crate) looks: watch::Sender<(InputWait, bool)>,
}

impl AdoptedWatch {
    /// Looks once per quiet period until the command ends, the shell ends, or a stop
    /// is done.
    pub(crate) async fn run(self) {
        let mut watch = InputWatch::default();
        loop {
            let next = self.clock.sleep(self.quiet_period);
            tokio::select! {
                () = next => {}
                _ = self.session.ended() => return,
            }
            let looked =
                self.session.look(self.id, self.offer, self.quiet, &*self.clock, &mut watch).await;
            match looked {
                Ok(Looked::Same) => continue,
                Ok(Looked::Changed { wait, told: Ok(()) }) => {
                    self.looks.send_replace((wait, watch.looks_secret()));
                }
                Ok(Looked::Gone | Looked::Changed { told: Err(_), .. }) | Err(_) => return,
            }
            let wait = watch.current();
            let secret =
                wait == InputWait::Hidden || (wait == InputWait::Visible && watch.looks_secret());
            if secret {
                self.interrupt(watch.waiting().map(|waiting| waiting.group)).await;
                return;
            }
        }
    }

    /// Stops the command, which waits for a secret that nobody can answer: `SIGINT` to
    /// the job that waits, while it holds the terminal. The shell stays: the job closes
    /// it when the command does not end, after it killed every process below the shell,
    /// also one that left the shell's session.
    async fn interrupt(&self, group: Option<u32>) {
        let session = &self.session;
        let conversation = session.conversation;
        // NOTE: the group is read right before the signal, as for a command left
        // running: once the command ended, zsh holds the terminal and runs its precmd
        // hooks, where a signal could cut short the hook that prints `D`.
        match group.map(|group| session.terminal.holds(group)) {
            Some(Ok(true)) => {
                tracing::info!(%conversation, "a command that a job adopted waits for a secret that nobody can type; interrupting it");
                let signal = self
                    .holder
                    .signal(session.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)
                    .await;
                if let Err(error) = signal {
                    tracing::warn!(%conversation, %error, "could not interrupt a command that a job adopted");
                }
            }
            Some(Ok(false)) | None => {
                tracing::debug!(%conversation, "the job that waited no longer holds the terminal; nothing is interrupted");
            }
            Some(Err(error)) => {
                tracing::warn!(%conversation, %error, "could not read who holds the terminal of an adopted shell");
            }
        }
    }
}

/// What the manager learns from the actor when it adopts a run.
#[derive(Debug)]
pub(crate) struct Adoption {
    pub(crate) kept: Kept,
    pub(crate) range: Option<Range<Seq>>,
    /// The shell's own directory at the move.
    pub(crate) cwd: PathBuf,
    /// The call's dir of a sandboxed call.
    pub(crate) call_dir: Option<PathBuf>,
}

#[cfg(test)]
mod tests;
