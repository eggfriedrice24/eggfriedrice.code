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

/// How much one read takes at most.
const READ_CHUNK: usize = 16 * 1024;

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
    Ok(Arc::new(AsyncFd::new(master)?))
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
pub(crate) async fn read_loop(master: Master, targets: ReaderTargets) {
    let mut buf = vec![0; READ_CHUNK];
    let mut offset: u64 = 0;
    loop {
        let n = match read_some(&master, &mut buf).await {
            // EIO on a master means no process holds the slave open any more.
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let bytes = Bytes::copy_from_slice(&buf[..n]);
        let start = Seq::new(offset);
        offset = offset.saturating_add(n as u64);
        targets.recording.record(targets.pty_id, start, bytes.clone()).await;
        if targets.session.send(Msg::Chunk { start, bytes: bytes.clone() }).await.is_err() {
            break;
        }
        // A screen that stopped only loses the view; the shell goes on.
        let _ = targets.screen.feed(bytes, start).await;
    }
}

async fn read_some(master: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let mut ready = master.readable().await?;
        match ready.try_io(|fd| rustix::io::read(fd.get_ref(), &mut *buf).map_err(io::Error::from))
        {
            Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
            Ok(result) => return result,
            // Not readable after all; tokio cleared the readiness, so wait again.
            Err(_would_block) => {}
        }
    }
}
