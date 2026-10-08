//! Keys from the terminal: one-key answers to approval requests, the bytes of an
//! answer line for a command that waits for input (`crate::answer` edits the line), and
//! the keys of the input row of a turn (`crate::row` edits that one).
//!
//! While keys are read, a named thread puts the terminal on stdin into non-canonical
//! mode without echo, so a single key arrives without Enter and does not show: a
//! password typed for a command never appears, and the CLI echoes a visible answer
//! itself. Signals stay on, so Ctrl+C still interrupts the turn. The read times out
//! every tenth of a second (`VMIN` 0, `VTIME` 1), so the thread notices a stop request
//! without a signal, restores the terminal's settings itself, and only then reports
//! that it is done. Nothing therefore exits while the terminal is still in that mode,
//! and no read is left behind to swallow a line the user types into the shell later.
//!
//! Keys typed before the question appeared are discarded first: a stray key from
//! earlier never answers it, and text typed earlier, such as a password typed blind
//! while a command waited, is not left for the shell to read after `efr` exits. A
//! reader that read an answer line also discards what is still unread when it stops,
//! before echo comes back, so the rest of a password never shows or reaches the shell.
//!
//! The input row of a turn reads keys for the whole turn ([`Keys::keep`]): its reader
//! keeps the typeahead, because the user typed it for the row before the row showed.
//! The terminal's map of carriage return to newline (`ICRNL`) is off, so Enter (a
//! carriage return) and Ctrl+J (a newline) stay two keys. An escape byte that no other
//! byte follows within one read timeout is a press of Esc ([`Key::Esc`]): a terminal
//! sends the bytes of an escape sequence in one write, so they come together.
//! [`KeyReader::flush`] throws away what was typed so far without leaving the mode, for
//! the moment a password line gives the keys back to the row.

use std::fmt;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use efr_protocol::ApprovalDecision;
use rustix::termios::{
    InputModes, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios, tcflush,
    tcgetattr, tcsetattr,
};
use tokio::sync::{mpsc, oneshot};

use crate::error::CliError;

/// The stack of the key thread; its loop is one read deep.
const THREAD_STACK: usize = 64 * 1024;

/// Keys waiting for the consumer; a person types far slower than this drains.
const KEY_QUEUE: usize = 16;

/// The escape byte, which starts an escape sequence or is the Esc key alone.
pub(crate) const ESC: u8 = 0x1b;

/// The answer a key gives, if any: `y` allows, `n` denies, anything else is ignored.
pub(crate) fn decision(key: u8) -> Option<ApprovalDecision> {
    match key {
        b'y' | b'Y' => Some(ApprovalDecision::Allow),
        b'n' | b'N' => Some(ApprovalDecision::Deny),
        _ => None,
    }
}

/// One key from the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// One byte of what was typed: a character, a part of one, or a part of an escape
    /// sequence.
    Byte(u8),
    /// The Esc key alone: an escape byte that no other byte followed within one read
    /// timeout.
    Esc,
}

impl Key {
    /// The byte of the key. Esc is the escape byte, as a line that reads bytes sees it.
    pub(crate) fn byte(self) -> u8 {
        match self {
            Key::Byte(byte) => byte,
            Key::Esc => ESC,
        }
    }
}

/// What the key thread sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Read {
    /// A key.
    Key(Key),
    /// The thread threw away the unread input, as [`KeyReader::flush`] asked: every
    /// key before this one was typed before the flush.
    Flushed,
}

/// Where keys come from.
pub(crate) trait Keys: Send + Sync + fmt::Debug {
    /// True when there is a terminal to read keys from.
    fn available(&self) -> bool;

    /// Starts reading single keys, after it throws away the keys typed before.
    fn start(&self) -> Result<KeyReader, CliError>;

    /// Starts reading single keys and keeps the keys typed before: the input row reads
    /// what the user typed while `efr` started.
    fn keep(&self) -> Result<KeyReader, CliError>;
}

/// The flags that the consumer sets and the key thread reads.
#[derive(Debug, Default)]
struct Flags {
    stop: AtomicBool,
    /// Set before `stop`: input that is still unread goes when the terminal is restored.
    discard: AtomicBool,
    /// Throw away the unread input now, and send [`Read::Flushed`].
    flush: AtomicBool,
    /// Set the key mode again: a stop (Ctrl+Z) gave the terminal to the shell, which
    /// may have changed it.
    reapply: AtomicBool,
}

/// Keys as they are typed, until [`stop`](KeyReader::stop).
#[derive(Debug)]
pub(crate) struct KeyReader {
    keys: mpsc::Receiver<Read>,
    flags: Arc<Flags>,
    /// A thread reads the keys and sends [`Read::Flushed`] after a flush; the reader of
    /// a test's channel has none.
    threaded: bool,
    /// A flush was asked for and the thread did not confirm it yet: the keys until then
    /// are thrown away.
    flushing: bool,
    done: oneshot::Receiver<()>,
}

impl KeyReader {
    /// A reader over `keys` whose source sets nothing up, for tests.
    #[cfg(test)]
    pub(crate) fn from_channel(keys: mpsc::Receiver<Read>) -> KeyReader {
        let (finished, done) = oneshot::channel();
        drop(finished);
        KeyReader { keys, flags: Arc::default(), threaded: false, flushing: false, done }
    }

    /// Whether the reader was stopped with its unread input thrown away, for tests.
    #[cfg(test)]
    pub(crate) fn discard_flag(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let flags = Arc::clone(&self.flags);
        Arc::new(move || flags.discard.load(Ordering::Acquire))
    }

    /// The next key; `None` once the source has ended.
    pub(crate) async fn next(&mut self) -> Option<Key> {
        loop {
            match self.keys.recv().await? {
                Read::Flushed => self.flushing = false,
                Read::Key(_) if self.flushing => {}
                Read::Key(key) => return Some(key),
            }
        }
    }

    /// Drops the keys that wait in the queue, so that a new question is answered only
    /// by keys typed after it appeared, as a new reader's flush would do.
    pub(crate) fn discard_queued(&mut self) {
        while self.queued().is_some() {}
    }

    /// The next key that already waits in the queue, without waiting for one.
    pub(crate) fn queued(&mut self) -> Option<Key> {
        loop {
            match self.keys.try_recv().ok()? {
                Read::Flushed => self.flushing = false,
                Read::Key(_) if self.flushing => {}
                Read::Key(key) => return Some(key),
            }
        }
    }

    /// Throws away every key typed so far, also the keys that the thread did not read
    /// yet, and goes on reading in the same mode: the rest of a password typed for a
    /// command must not reach the input row.
    pub(crate) fn flush(&mut self) {
        self.discard_queued();
        if self.threaded {
            self.flags.flush.store(true, Ordering::Release);
            self.flushing = true;
        }
    }

    /// The process runs again after a stop: the thread sets its mode again, in case
    /// the shell changed the terminal meanwhile.
    pub(crate) fn resumed(&self) {
        self.flags.reapply.store(true, Ordering::Release);
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
        self.flags.discard.store(true, Ordering::Release);
        self.finish().await;
    }

    async fn finish(self) {
        let KeyReader { keys, flags, done, .. } = self;
        flags.stop.store(true, Ordering::Release);
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
        start_on(stdin()?, Typeahead::Discard)
    }

    fn keep(&self) -> Result<KeyReader, CliError> {
        start_on(stdin()?, Typeahead::Keep)
    }
}

/// A descriptor of the terminal on stdin, for the key thread.
fn stdin() -> Result<OwnedFd, CliError> {
    rustix::io::dup(io::stdin())
        .map_err(|errno| CliError::Terminal { source: io::Error::from(errno) })
}

/// What happens to the keys typed before the key thread starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typeahead {
    /// They are thrown away: a question is answered only by keys typed after it.
    Discard,
    /// They are read first: the input row takes them.
    Keep,
}

/// Starts the key thread on the terminal `fd`.
pub(crate) fn start_on(fd: OwnedFd, typeahead: Typeahead) -> Result<KeyReader, CliError> {
    let (sender, keys) = mpsc::channel(KEY_QUEUE);
    let (finished, done) = oneshot::channel();
    let flags = Arc::new(Flags::default());
    let thread_flags = Arc::clone(&flags);
    efr_stdx::thread::spawn_named("efr-keys", THREAD_STACK, move || {
        if let Err(error) = read_keys(&fd, &sender, &thread_flags, typeahead) {
            tracing::debug!(%error, "reading keys from the terminal failed");
        }
        drop(finished);
    })
    .map_err(|source| CliError::Terminal { source: io::Error::other(source) })?;
    Ok(KeyReader { keys, flags, threaded: true, flushing: false, done })
}

/// Reads keys from `fd` in non-canonical mode until `stop` is set or the consumer is
/// gone, then restores the terminal's settings, first throwing away unread input when
/// `discard` is set.
fn read_keys(
    fd: &OwnedFd,
    keys: &mpsc::Sender<Read>,
    flags: &Flags,
    typeahead: Typeahead,
) -> io::Result<()> {
    let saved = tcgetattr(fd)?;
    let mut keyed = saved.clone();
    keyed.local_modes.remove(LocalModes::ICANON | LocalModes::ECHO);
    // Enter sends a carriage return and Ctrl+J a newline; the map would make them one.
    keyed.input_modes.remove(InputModes::ICRNL);
    keyed.special_codes[SpecialCodeIndex::VMIN] = 0;
    keyed.special_codes[SpecialCodeIndex::VTIME] = 1;
    if typeahead == Typeahead::Discard {
        // Typeahead goes first, so once the mode has changed every key read was typed
        // after the question appeared.
        tcflush(fd, QueueSelector::IFlush)?;
    }
    tcsetattr(fd, OptionalActions::Now, &keyed)?;
    let _restore = Restore { fd: fd.as_fd(), saved, discard: &flags.discard };
    while !flags.stop.load(Ordering::Acquire) {
        if flags.reapply.swap(false, Ordering::AcqRel) {
            tcsetattr(fd, OptionalActions::Now, &keyed)?;
        }
        if flags.flush.swap(false, Ordering::AcqRel) {
            tcflush(fd, QueueSelector::IFlush)?;
            if keys.blocking_send(Read::Flushed).is_err() {
                break;
            }
        }
        let Some(byte) = read_byte(fd)? else {
            continue;
        };
        let sent = if byte == ESC {
            // NOTE: the rest of an escape sequence comes in the same write; an escape
            // byte that nothing follows within one read timeout is the Esc key.
            match read_byte(fd)? {
                None => keys.blocking_send(Read::Key(Key::Esc)),
                Some(next) => keys
                    .blocking_send(Read::Key(Key::Byte(ESC)))
                    .and_then(|()| keys.blocking_send(Read::Key(Key::Byte(next)))),
            }
        } else {
            keys.blocking_send(Read::Key(Key::Byte(byte)))
        };
        if sent.is_err() {
            break;
        }
    }
    Ok(())
}

/// One byte from `fd`, or `None` when the read timeout (`VTIME`) passed without one.
fn read_byte(fd: &OwnedFd) -> io::Result<Option<u8>> {
    let mut byte = [0_u8; 1];
    match rustix::io::read(fd, &mut byte) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(byte[0])),
        Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => Ok(None),
        Err(errno) => Err(errno.into()),
    }
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
