//! Descriptor numbers that the process does not own yet: the one module of efr-sbx
//! that may contain `unsafe` code (ADR 0007, `docs/adr/0007-efr-sbx-unsafe.md`).
//!
//! Safe Rust owns a descriptor through an `OwnedFd` that it opened itself. The
//! launcher and the inner stage also handle descriptors that arrive by number: the ones
//! the trusted shell left open, the ones bwrap passes to `efr-sbx inner` (the policy,
//! the records pipe, the terminal copy), and fd 3 of the child, which must be the
//! records pipe at exactly that number. rustix has no `close_range` and no way to turn a
//! number into an `OwnedFd`, so these few steps call `libc` here:
//!
//! - [`mark_inherited_cloexec`]: `close_range(first, ~0, CLOSE_RANGE_CLOEXEC)`, so no
//!   descriptor of the trusted shell reaches bwrap or the exit child.
//! - [`close_from`]: `close_range(first, ~0, 0)` in the inner stage, before Landlock, so
//!   no descriptor opened outside the sandbox keeps its rights inside.
//! - [`adopt`]: an `OwnedFd` for a number that an argument names, after `fcntl`
//!   checked that it is open.
//! - [`dup_to`]: `dup3` onto a fixed number that nothing in the process owns.
//! - [`inherit_number`]: clears `FD_CLOEXEC` on a bind source that
//!   `efr_sandbox::FdTable` owns and names only by number.
//! - [`io_uring_blocked`], [`tiocsti_blocked`] and [`userns_blocked`]: the probe's
//!   self-test makes these three system calls to prove that seccomp refuses them;
//!   rustix offers them only as `unsafe` or not at all.
//!
//! Every block names what it relies on in a `// SAFETY:` comment. Each function is
//! called while the process has one thread: at the start of the launcher, before the
//! reader threads exist, and in the inner stage, which never starts a thread.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

/// Marks every descriptor from `first` up close-on-exec.
pub(crate) fn mark_inherited_cloexec(first: RawFd) -> io::Result<()> {
    close_range(first, libc::CLOSE_RANGE_CLOEXEC)
}

/// Closes every descriptor from `first` up. Any `OwnedFd` of such a number must be
/// forgotten or dropped before, so nothing closes the number again later.
pub(crate) fn close_from(first: RawFd) -> io::Result<()> {
    close_range(first, 0)
}

fn close_range(first: RawFd, flags: libc::c_uint) -> io::Result<()> {
    let first = u32::try_from(first).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: close_range takes three integers and touches no memory. With
    // CLOSE_RANGE_CLOEXEC it only sets a flag in this process's descriptor table; with
    // no flag it closes descriptors that, by the callers' contract, no `OwnedFd` of this
    // process holds any more (`close_from`'s doc).
    let done = unsafe { libc::syscall(libc::SYS_close_range, first, u32::MAX, flags) };
    if done == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Clears `FD_CLOEXEC` on `fd`, a bind source that `efr_sandbox::FdTable` opened and
/// owns: the table hands out numbers, not borrows, and keeps the descriptor open until
/// it drops after bwrap started.
pub(crate) fn inherit_number(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl(F_SETFD) takes integers and touches no memory. The caller's table
    // keeps `fd` open for this call, so the flag lands on that descriptor and no other.
    let done = unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
    if done == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Takes ownership of the open descriptor `fd`, which an argument named. Refuses the
/// standard streams and a number that is not open.
///
/// The caller passes each number once, and only a number that nothing else in the
/// process owns: a descriptor inherited from the parent and named on the command line.
pub(crate) fn adopt(fd: RawFd) -> io::Result<OwnedFd> {
    if fd < 3 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    // SAFETY: fcntl(F_GETFD) takes integers and touches no memory; on a number that is
    // not open it fails with EBADF.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is open (checked above) and, by this function's contract, no other
    // `OwnedFd` or `File` of this process owns it, so this is its one owner.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// A copy of `src` at exactly `target`, close-on-exec when `cloexec`; a descriptor
/// already at `target` is closed by the copy.
///
/// The caller guarantees that no `OwnedFd` of this process owns `target`, so the copy
/// has one owner, the returned one.
pub(crate) fn dup_to(src: BorrowedFd<'_>, target: RawFd, cloexec: bool) -> io::Result<OwnedFd> {
    if target < 3 || src.as_raw_fd() == target {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let flags = if cloexec { libc::O_CLOEXEC } else { 0 };
    // SAFETY: dup3 takes integers and touches no memory. `src` is open for the life of
    // the borrow, the two numbers differ (checked above), and `target` is not owned by
    // anything else in this process (this function's contract), so closing a descriptor
    // that was there cannot pull it from under an owner.
    let done = unsafe { libc::dup3(src.as_raw_fd(), target, flags) };
    if done < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: dup3 returned `target`, which is now open and has no other owner.
    Ok(unsafe { OwnedFd::from_raw_fd(target) })
}

/// The self-test's `io_uring_setup(1, &params)`: `Ok(errno)` when it failed as seccomp
/// makes it fail, `Err` with what happened otherwise.
pub(crate) fn io_uring_blocked() -> Result<i32, String> {
    // The kernel's struct io_uring_params is 120 bytes; zeroed storage of that size is
    // what io_uring_setup expects for default parameters.
    let mut params = [0_u8; 120];
    // SAFETY: io_uring_setup reads and writes exactly the 120 bytes of `params`, which
    // is a live, writable buffer of that size for the whole call. A ring that the call
    // makes is closed below and maps no memory, because nothing calls mmap on it.
    let done = unsafe { libc::syscall(libc::SYS_io_uring_setup, 1_u32, params.as_mut_ptr()) };
    if done >= 0 {
        let fd = RawFd::try_from(done).unwrap_or(-1);
        if fd >= 0 {
            // SAFETY: the call returned a new descriptor that nothing else owns.
            drop(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        return Err("io_uring_setup made a ring".to_owned());
    }
    Ok(io::Error::last_os_error().raw_os_error().unwrap_or_default())
}

/// The self-test's `ioctl(fd, TIOCSTI, "x")` on a terminal: `Ok(errno)` when it failed,
/// `Err` when a byte went into the terminal's input.
pub(crate) fn tiocsti_blocked(terminal: BorrowedFd<'_>) -> Result<i32, String> {
    let byte: libc::c_char = 0;
    // SAFETY: TIOCSTI reads one byte through its pointer argument, which points to a
    // live local; the descriptor stays open for the borrow. On success it pushes that
    // NUL byte into the input of a terminal that the self-test opened itself.
    let done = unsafe { libc::ioctl(terminal.as_raw_fd(), libc::TIOCSTI, &raw const byte) };
    if done == 0 {
        return Err("TIOCSTI pushed a byte into the terminal".to_owned());
    }
    Ok(io::Error::last_os_error().raw_os_error().unwrap_or_default())
}

/// The self-test's `unshare(CLONE_NEWUSER)`: `Ok(errno)` when it failed, `Err` when
/// the process got a user namespace of its own.
pub(crate) fn userns_blocked() -> Result<i32, String> {
    // SAFETY: unshare takes one integer and touches no memory. CLONE_NEWUSER alone does
    // not unshare the descriptor table, which is what makes rustix mark it unsafe; the
    // self-test has one thread and exits right after its checks.
    let done = unsafe { libc::unshare(libc::CLONE_NEWUSER) };
    if done == 0 {
        return Err("unshare made a user namespace".to_owned());
    }
    Ok(io::Error::last_os_error().raw_os_error().unwrap_or_default())
}

#[cfg(test)]
mod tests;
