//! The PTY master as tokio sees it, and the task that reads the shell's output.

use std::io;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use bytes::Bytes;
use efr_protocol::{PtyId, Seq};
use efr_screen::ScreenHandle;
use rustix::fs::OFlags;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

use crate::RecordingSink;
use crate::session::Msg;

/// The most bytes that the reader hands on as one chunk. A PTY gives a few KB a read
/// while a program prints fast, so a chunk of many reads costs the recording, the
/// session and the screen one step each, not one step per read.
pub(crate) const BATCH: usize = 64 * 1024;

/// The master in non-blocking mode inside tokio's reactor, shared by the reader and
/// the writer.
pub(crate) type Master = Arc<AsyncFd<OwnedFd>>;

/// Puts `master` into non-blocking mode and registers it with the reactor.
///
/// The holder hands the master over in blocking mode. The flag lives on the open file
/// description, which the holder's own copy shares; the holder only runs ioctls on its
/// copy, so the change is safe. It needs a tokio runtime with IO enabled.
pub(crate) fn master(master: OwnedFd) -> io::Result<Master> {
    tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
    let flags = rustix::fs::fcntl_getfl(&master)?;
    rustix::fs::fcntl_setfl(&master, flags | OFlags::NONBLOCK)?;
    // NOTE: tokio 1.53.3 deprecates AsyncFd::new (RUSTSEC-2026-0386) for a borrowed
    // descriptor. This AsyncFd owns its OwnedFd, so no other code can close or replace
    // the descriptor while it is registered, which is the condition that
    // `AsyncFd::register` asks its caller to keep.
    #[expect(deprecated, reason = "the AsyncFd owns the descriptor")]
    let master = AsyncFd::new(master)?;
    Ok(Arc::new(master))
}

/// What the reader hands each chunk to.
#[derive(Debug)]
pub(crate) struct ReaderTargets {
    pub(crate) pty_id: PtyId,
    pub(crate) recording: Arc<dyn RecordingSink>,
    pub(crate) session: mpsc::Sender<Msg>,
    pub(crate) screen: ScreenHandle,
}

/// Reads the shell's output until the PTY closes. Each chunk goes to the recording
/// first, then to the session (marks, runs), then to the screen. Every step waits, so
/// a slow consumer slows the reads and, through the kernel's buffer, the shell.
///
/// A chunk is what the master holds when the reader looks, up to [`BATCH`]: the reader
/// waits for the first bytes, then reads on until the master has no more. It never
/// waits for more output, so a prompt goes on as soon as it is read.
pub(crate) async fn read_loop(master: Master, targets: ReaderTargets) {
    // NOTE: the reads go into one buffer, and each chunk is one copy of its bytes. A
    // chunk that shares a larger buffer would keep all of that buffer alive while an
    // attached client's queue holds the chunk.
    let mut buf = vec![0; BATCH];
    let mut offset: u64 = 0;
    loop {
        let Batch { len, end } = read_batch(&master, &mut buf).await;
        if len > 0 {
            let bytes = Bytes::copy_from_slice(&buf[..len]);
            let start = Seq::new(offset);
            offset = offset.saturating_add(len as u64);
            targets.recording.record(targets.pty_id, start, bytes.clone()).await;
            if targets.session.send(Msg::Chunk { start, bytes: bytes.clone() }).await.is_err() {
                break;
            }
            // A screen that stopped only loses the view; the shell goes on.
            let _ = targets.screen.feed(bytes, start).await;
        }
        if end {
            break;
        }
    }
}

/// What one batch of reads took.
struct Batch {
    /// The bytes read, at the start of the buffer.
    len: usize,
    /// The PTY closed: EOF, or EIO, which on a master means that no process holds the
    /// slave open any more.
    end: bool,
}

/// Waits until the master is readable, then reads into `buf` until it is full or the
/// master has nothing more.
async fn read_batch(master: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> Batch {
    let mut len = 0;
    loop {
        let mut ready = match master.readable().await {
            Ok(ready) => ready,
            Err(_) => return Batch { len, end: true },
        };
        while len < buf.len() {
            let free = &mut buf[len..];
            match ready
                .try_io(|fd| rustix::io::read(fd.get_ref(), &mut *free).map_err(io::Error::from))
            {
                Ok(Ok(0)) => return Batch { len, end: true },
                Ok(Ok(n)) => len += n,
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(_)) => return Batch { len, end: true },
                // Nothing more to read; tokio cleared the readiness. With bytes in
                // hand, they go on now; without, the reader waits again.
                Err(_would_block) if len > 0 => return Batch { len, end: false },
                Err(_would_block) => break,
            }
        }
        if len == buf.len() {
            return Batch { len, end: false };
        }
    }
}

#[cfg(test)]
mod tests;
