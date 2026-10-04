//! The task that writes to the shell: typed command lines, input from attached
//! clients and the screen's answers to terminal queries.

use std::collections::VecDeque;
use std::io;
use std::os::fd::OwnedFd;

use bytes::{Buf as _, Bytes};
use efr_screen::{ScreenEvent, ScreenEvents};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

use crate::reader::Master;

/// How many writes may wait in the channel. The writer moves them into its own queue
/// at once, so senders only wait while the writer task is not scheduled.
pub(crate) const WRITE_CAPACITY: usize = 64;

/// Writes everything sent on `inbox` to the master, in order, until every sender is
/// gone and the queue is written, or the PTY fails.
///
/// The channel is drained into a local queue even while the master is not writable
/// (the shell is not reading its input), so a sender never waits on the shell. That
/// keeps the screen's reply path from closing a loop with the reader: reader feeds
/// screen, screen replies, writer blocks, shell blocks, reader starves.
pub(crate) async fn write_loop(master: Master, mut inbox: mpsc::Receiver<Bytes>) {
    let mut queue: VecDeque<Bytes> = VecDeque::new();
    loop {
        let Some(front) = queue.front().cloned() else {
            match inbox.recv().await {
                Some(bytes) => queue.push_back(bytes),
                None => return,
            }
            continue;
        };
        tokio::select! {
            biased;
            received = inbox.recv() => match received {
                Some(bytes) => queue.push_back(bytes),
                None => {
                    drain(&master, &mut queue).await;
                    return;
                }
            },
            written = write_some(&master, &front) => match written {
                Ok(n) => advance(&mut queue, n),
                Err(_) => return,
            },
        }
    }
}

/// Writes what is left once nobody sends any more.
async fn drain(master: &AsyncFd<OwnedFd>, queue: &mut VecDeque<Bytes>) {
    while let Some(front) = queue.front().cloned() {
        match write_some(master, &front).await {
            Ok(n) => advance(queue, n),
            Err(_) => return,
        }
    }
}

fn advance(queue: &mut VecDeque<Bytes>, mut written: usize) {
    while written > 0 {
        let Some(front) = queue.front_mut() else {
            return;
        };
        let step = written.min(front.len());
        front.advance(step);
        written -= step;
        if front.is_empty() {
            queue.pop_front();
        }
    }
    while queue.front().is_some_and(Bytes::is_empty) {
        queue.pop_front();
    }
}

/// Writes some of `bytes` once the master is writable. Cancel-safe: the write happens
/// inside the poll that returns its count, so a dropped future wrote nothing.
async fn write_some(master: &AsyncFd<OwnedFd>, bytes: &[u8]) -> io::Result<usize> {
    if bytes.is_empty() {
        return Ok(0);
    }
    loop {
        let mut ready = master.writable().await?;
        match ready.try_io(|fd| rustix::io::write(fd.get_ref(), bytes).map_err(io::Error::from)) {
            Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
            Ok(result) => return result,
            Err(_would_block) => {}
        }
    }
}

/// Sends the screen's answers to terminal queries to the writer, and drops every
/// other screen event: the session reads the marks from the stream itself. It keeps
/// reading after the writer is gone, so the screen never waits on it.
pub(crate) async fn forward_replies(mut events: ScreenEvents, writer: mpsc::Sender<Bytes>) {
    while let Some(event) = events.recv().await {
        if let ScreenEvent::PtyReply(bytes) = event {
            let _ = writer.send(bytes).await;
        }
    }
}

#[cfg(test)]
mod tests;
