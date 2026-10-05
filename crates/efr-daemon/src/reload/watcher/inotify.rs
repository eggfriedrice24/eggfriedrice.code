//! The inotify descriptor the config watcher reads: rustix's safe inotify calls on a
//! non-blocking, close-on-exec descriptor that tokio's [`AsyncFd`] polls.
//!
//! efrd runs on Linux only, so inotify is the one backend. A directory watch asks only
//! for the events that can change a file in it ([`WATCHED`]): no open, no read and no
//! close after a read, so each `efr` command and each reload, which read the file, never
//! wake the watcher.

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;
use tokio::io::unix::AsyncFd;

/// The events a directory watch asks for. A save by rename (vim, nvim) is a create or a
/// move to; a save in place ends with a close after a write; a removal is a delete or a
/// move from. The watched directory itself going away or moving makes the watch stale.
pub(crate) const WATCHED: WatchFlags = WatchFlags::CREATE
    .union(WatchFlags::CLOSE_WRITE)
    .union(WatchFlags::DELETE)
    .union(WatchFlags::MOVED_FROM)
    .union(WatchFlags::MOVED_TO)
    .union(WatchFlags::DELETE_SELF)
    .union(WatchFlags::MOVE_SELF)
    .union(WatchFlags::ONLYDIR);

/// The bytes one read may fill. One event takes 16 bytes and a name of at most 256, so
/// a read always fits at least one, and usually a whole burst.
const BUFFER: usize = 4096;

/// One event of a watch, copied out of the read buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Event {
    /// The watch it belongs to; -1 for a queue overflow.
    pub(crate) wd: i32,
    /// What happened.
    pub(crate) flags: ReadFlags,
    /// The name of the entry in the watched directory, when the event is about one.
    pub(crate) name: Option<OsString>,
}

/// An inotify instance whose events are read without blocking a worker.
#[derive(Debug)]
pub(crate) struct Inotify {
    fd: AsyncFd<OwnedFd>,
}

impl Inotify {
    /// A new instance with no watches. Needs a tokio runtime with I/O enabled.
    pub(crate) fn new() -> io::Result<Self> {
        // NOTE: close-on-exec keeps the descriptor out of the hidden shells and the
        // programs they start.
        let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)?;
        Ok(Inotify { fd: AsyncFd::new(fd)? })
    }

    /// Watches the directory `dir` for [`WATCHED`] events and returns the watch. A
    /// directory already watched under another path returns the same watch.
    pub(crate) fn add(&self, dir: &Path) -> io::Result<i32> {
        Ok(inotify::add_watch(self.fd.get_ref(), dir, WATCHED)?)
    }

    /// Stops the watch `wd`. The kernel then queues an `IN_IGNORED` event for it.
    pub(crate) fn remove(&self, wd: i32) -> io::Result<()> {
        Ok(inotify::remove_watch(self.fd.get_ref(), wd)?)
    }

    /// Waits for events and returns all that are queued, at least one.
    pub(crate) async fn next(&self) -> io::Result<Vec<Event>> {
        loop {
            let mut ready = self.fd.readable().await?;
            let mut events = Vec::new();
            // NOTE: readiness is cleared only once a read found the queue empty, as
            // AsyncFd asks; otherwise the next call reads at once.
            if read_queued(ready.get_inner(), &mut events)? {
                ready.clear_ready();
            }
            if !events.is_empty() {
                return Ok(events);
            }
        }
    }
}

/// Reads what is queued on `fd` into `events`. True when the queue is empty now. An
/// error comes back only when nothing was read; otherwise the events read so far are
/// handed out, and the error comes again at the next read if it stays.
fn read_queued(fd: &OwnedFd, events: &mut Vec<Event>) -> io::Result<bool> {
    let mut buf = [MaybeUninit::<u8>::uninit(); BUFFER];
    let mut reader = inotify::Reader::new(fd, &mut buf);
    loop {
        match reader.next() {
            Ok(event) => events.push(Event {
                wd: event.wd(),
                flags: event.events(),
                name: event.file_name().map(|name| OsStr::from_bytes(name.to_bytes()).to_owned()),
            }),
            Err(Errno::WOULDBLOCK) => return Ok(true),
            Err(Errno::INTR) => {}
            Err(error) if events.is_empty() => return Err(error.into()),
            Err(_) => return Ok(false),
        }
    }
}

#[cfg(test)]
mod tests;
