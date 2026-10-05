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
//! while a command waited, is not left for the shell to read after `efr` exits.

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
    done: oneshot::Receiver<()>,
}

impl KeyReader {
    /// A reader over `keys` whose source sets nothing up, for tests.
    #[cfg(test)]
    pub(crate) fn from_channel(keys: mpsc::Receiver<u8>) -> KeyReader {
        let (finished, done) = oneshot::channel();
        drop(finished);
        KeyReader { keys, stop: Arc::new(AtomicBool::new(false)), done }
    }

    /// The next key; `None` once the source has ended.
    pub(crate) async fn next(&mut self) -> Option<u8> {
        self.keys.recv().await
    }

    /// Stops reading and waits until the terminal's settings are restored.
    pub(crate) async fn stop(self) {
        self.stop.store(true, Ordering::Release);
        // The thread drops its end once it has restored the terminal; an error only
        // means it is gone.
        let _ = self.done.await;
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
    let thread_stop = Arc::clone(&stop);
    efr_stdx::thread::spawn_named("efr-keys", THREAD_STACK, move || {
        if let Err(error) = read_keys(&fd, &sender, &thread_stop) {
            tracing::debug!(%error, "reading keys from the terminal failed");
        }
        drop(finished);
    })
    .map_err(|source| CliError::Terminal { source: io::Error::other(source) })?;
    Ok(KeyReader { keys, stop, done })
}

/// Reads keys from `fd` in non-canonical mode until `stop` is set or the consumer is
/// gone, then restores the terminal's settings.
fn read_keys(fd: &OwnedFd, keys: &mpsc::Sender<u8>, stop: &AtomicBool) -> io::Result<()> {
    let saved = tcgetattr(fd)?;
    let mut keyed = saved.clone();
    keyed.local_modes.remove(LocalModes::ICANON | LocalModes::ECHO);
    keyed.special_codes[SpecialCodeIndex::VMIN] = 0;
    keyed.special_codes[SpecialCodeIndex::VTIME] = 1;
    // Typeahead goes first, so once the mode has changed every key read was typed
    // after the question appeared.
    tcflush(fd, QueueSelector::IFlush)?;
    tcsetattr(fd, OptionalActions::Now, &keyed)?;
    let _restore = Restore { fd: fd.as_fd(), saved };
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
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if let Err(error) = tcsetattr(self.fd, OptionalActions::Now, &self.saved) {
            tracing::warn!(%error, "the terminal's settings could not be restored");
        }
    }
}

#[cfg(test)]
mod tests;
