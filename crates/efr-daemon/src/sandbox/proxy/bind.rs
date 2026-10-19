//! The bind of a call's proxy socket (DP13).
//!
//! The path `$CALL/proxy.sock` can be longer than a socket address holds (107 bytes):
//! a job's call dir below `/run/user/1000/efr/sbx/<conversation>/jobs/<call>` is 112
//! bytes, and a daemon test's dirs are longer still. So efrd binds through
//! `/proc/self/fd/<dirfd>/proxy.sock`, with an `O_PATH` descriptor of the call dir: a
//! short path that names the same file, and no `chdir` (efrd has many threads). The
//! launcher binds the socket into the call read-only, so the mode 0600 here is only
//! for the processes outside.

use std::io;
use std::os::fd::AsRawFd as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use efr_sandbox::PROXY_SOCKET_FILE;
use rustix::fs::{AtFlags, Mode, OFlags};

/// Binds `call_dir/proxy.sock` (mode 0600) and returns the listener, ready for tokio.
/// The call dir must exist and hold no socket yet. It blocks.
pub(crate) fn bind_listener(call_dir: &Path) -> io::Result<UnixListener> {
    let dir = rustix::fs::open(
        call_dir,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let short = PathBuf::from(format!("/proc/self/fd/{}/{PROXY_SOCKET_FILE}", dir.as_raw_fd()));
    let listener = UnixListener::bind(&short)?;
    rustix::fs::chmodat(&dir, PROXY_SOCKET_FILE, Mode::from_raw_mode(0o600), AtFlags::empty())?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}
