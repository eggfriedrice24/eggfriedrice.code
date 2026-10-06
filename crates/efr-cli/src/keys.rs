//! Keys from the terminal: one-key answers to approval requests, and the bytes of an
//! answer line for a command that waits for input (`crate::answer` edits the line).
//!
//! While a question waits, a named thread puts the terminal on stdin into
//! non-canonical mode without echo, so a single key arrives without Enter and does not
//! show: a password typed for a command never appears, and the CLI echoes a visible
//! answer itself. Signals stay on, so Ctrl+C still interrupts the turn. The read times
//! out every tenth of a second (`VMIN` 0, `VTIME` 1), so the thread notices a stop
//! request without a signal, restores the terminal's settings itself, and only then
//! reports that it is done. Nothing therefore exits while the terminal is still in that
//! mode, and no read is left behind to swallow a line the user types into the shell
//! later.
//!
//! Keys typed before the question appeared are discarded first: a stray key from
//! earlier never answers it, and text typed earlier, such as a password typed blind
//! while a command waited, is not left for the shell to read after `efr` exits. A
//! reader that read an answer line also discards what is still unread when it stops,
//! before echo comes back, so the rest of a password never shows or reaches the shell.

use std::fmt;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use efr_protocol::ApprovalDecision;
use rustix::termios::{
    LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios, tcflush, tcgetattr,
    tcsetattr,
};
use tokio::sync::{mpsc, oneshot};

use crate::error::CliError;

/// The stack of the key thread; its loop is one read deep.
const THREAD_STACK: usize = 64 * 1024;

/// Keys waiting for the consumer; a person types far slower than this drains.
const KEY_QUEUE: usize = 16;

/// The answer a key gives, if any: `y` allows, `n` denies, anything else is ignored.
pub(crate) fn decision(key: u8) -> Option<ApprovalDecision> {
    match key {
        b'y' | b'Y' => Some(ApprovalDecision::Allow),
        b'n' | b'N' => Some(ApprovalDecision::Deny),
        _ => None,
    }
}

/// Where one-key answers come from.
pub(crate) trait Keys: Send + Sync + fmt::Debug {
    /// True when there is a terminal to read keys from.
    fn available(&self) -> bool;

    /// Starts reading single keys.
    fn start(&self) -> Result<KeyReader, CliError>;
}

/// Keys as they are typed, until [`stop`](KeyReader::stop).
#[derive(Debug)]
pub(crate) struct KeyReader {
    keys: mpsc::Receiver<u8>,
    stop: Arc<AtomicBool>,
    /// Set before `stop`: input that is still unread goes when the terminal is restored.
    discard: Arc<AtomicBool>,
    done: oneshot::Receiver<()>,
}

impl KeyReader {
    /// A reader over `keys` whose source sets nothing up, for tests.
    #[cfg(test)]
    pub(crate) fn from_channel(keys: mpsc::Receiver<u8>) -> KeyReader {
        let (finished, done) = oneshot::channel();
        drop(finished);
        KeyReader {
            keys,
            stop: Arc::new(AtomicBool::new(false)),
            discard: Arc::new(AtomicBool::new(false)),
            done,
        }
    }

    /// The flag that [`stop_discarding`](Self::stop_discarding) sets, for tests.
    #[cfg(test)]
    pub(crate) fn discard_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.discard)
    }

    /// The next key; `None` once the source has ended.
    pub(crate) async fn next(&mut self) -> Option<u8> {
        self.keys.recv().await
    }

    /// Drops the keys that wait in the queue, so that a new question is answered only
    /// by keys typed after it appeared, as a new reader's flush would do.
    pub(crate) fn discard_queued(&mut self) {
        while self.keys.try_recv().is_ok() {}
    }

    /// The next key that already waits in the queue, without waiting for one.
    pub(crate) fn queued(&mut self) -> Option<u8> {
        self.keys.try_recv().ok()
    }

    /// Stops reading and waits until the terminal's settings are restored; keys that
    /// were typed and not read yet stay for whatever reads the terminal next.
    pub(crate) async fn stop(self) {
        self.finish().await;
    }

    /// Stops reading like [`stop`](Self::stop), but throws away input that is still
    /// unread before echo comes back: the rest of a password typed for a command that
    /// stopped reading must neither show nor reach the user's shell after `efr` exits.
    pub(crate) async fn stop_discarding(self) {
        self.discard.store(true, Ordering::Release);
        self.finish().await;
    }

    async fn finish(self) {
        let KeyReader { keys, stop, discard: _, done } = self;
        stop.store(true, Ordering::Release);
        // NOTE: the queue goes first. A thread blocked on a full queue never looks at
        // the stop flag again; a closed queue ends its send, and so its loop.
        drop(keys);
        // The thread drops its end once it has restored the terminal; an error only
        // means it is gone.
        let _ = done.await;
    }
}

/// The terminal on stdin.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TtyKeys {
    /// Stdin is a terminal.
    pub(crate) available: bool,
}

impl Keys for TtyKeys {
    fn available(&self) -> bool {
        self.available
    }

    fn start(&self) -> Result<KeyReader, CliError> {
        let fd = rustix::io::dup(io::stdin())
            .map_err(|errno| CliError::Terminal { source: io::Error::from(errno) })?;
        start_on(fd)
    }
}

/// Starts the key thread on the terminal `fd`.
pub(crate) fn start_on(fd: OwnedFd) -> Result<KeyReader, CliError> {
    let (sender, keys) = mpsc::channel(KEY_QUEUE);
    let (finished, done) = oneshot::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let discard = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread_discard = Arc::clone(&discard);
    efr_stdx::thread::spawn_named("efr-keys", THREAD_STACK, move || {
        if let Err(error) = read_keys(&fd, &sender, &thread_stop, &thread_discard) {
            tracing::debug!(%error, "reading keys from the terminal failed");
        }
        drop(finished);
    })
    .map_err(|source| CliError::Terminal { source: io::Error::other(source) })?;
    Ok(KeyReader { keys, stop, discard, done })
}

/// Reads keys from `fd` in non-canonical mode until `stop` is set or the consumer is
/// gone, then restores the terminal's settings, first throwing away unread input when
/// `discard` is set.
fn read_keys(
    fd: &OwnedFd,
    keys: &mpsc::Sender<u8>,
    stop: &AtomicBool,
    discard: &AtomicBool,
) -> io::Result<()> {
    let saved = tcgetattr(fd)?;
    let mut keyed = saved.clone();
    keyed.local_modes.remove(LocalModes::ICANON | LocalModes::ECHO);
    keyed.special_codes[SpecialCodeIndex::VMIN] = 0;
    keyed.special_codes[SpecialCodeIndex::VTIME] = 1;
    // Typeahead goes first, so once the mode has changed every key read was typed
    // after the question appeared.
    tcflush(fd, QueueSelector::IFlush)?;
    tcsetattr(fd, OptionalActions::Now, &keyed)?;
    let _restore = Restore { fd: fd.as_fd(), saved, discard };
    let mut byte = [0_u8; 1];
    while !stop.load(Ordering::Acquire) {
        match rustix::io::read(fd, &mut byte) {
            // VTIME passed without a key.
            Ok(0) => {}
            Ok(_) => {
                if keys.blocking_send(byte[0]).is_err() {
                    break;
                }
            }
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
            Err(errno) => return Err(errno.into()),
        }
    }
    Ok(())
}

/// Puts the terminal's settings back when the key loop ends, however it ends.
struct Restore<'fd> {
    fd: std::os::fd::BorrowedFd<'fd>,
    saved: Termios,
    discard: &'fd AtomicBool,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        // NOTE: before the settings: once echo is back, a key that is still queued
        // would show, and after `efr` exits the user's shell would read it.
        if self.discard.load(Ordering::Acquire)
            && let Err(error) = tcflush(self.fd, QueueSelector::IFlush)
        {
            tracing::warn!(%error, "unread input could not be discarded");
        }
        if let Err(error) = tcsetattr(self.fd, OptionalActions::Now, &self.saved) {
            tracing::warn!(%error, "the terminal's settings could not be restored");
        }
    }
}

#[cfg(test)]
mod tests;
