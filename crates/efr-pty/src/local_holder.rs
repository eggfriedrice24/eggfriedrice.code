//! [`LocalPtyHolder`]: PTYs and their children inside the calling process.
//!
//! This is the one module in the workspace that may contain `unsafe` code, and it has
//! exactly two kinds of it, both for the child between `fork` and `execve`:
//!
//! - `pre_exec` itself is `unsafe`, because its closure runs in a copy of a
//!   multi-threaded process in which only async-signal-safe functions may be called.
//! - Resetting signal actions and the signal mask (`signal`, `sigprocmask`) and marking
//!   every other descriptor close-on-exec (`close_range`, and `fcntl` on kernels before
//!   5.11) have no rustix wrapper; they are called through `libc`.
//!
//! The sequence for one spawn, and why each step is where it is:
//!
//! 1. In the holder: open the master with `posix_openpt` (`O_NOCTTY | O_CLOEXEC`),
//!    `grantpt`, `unlockpt`, open the slave by its `ptsname` (also `O_NOCTTY`, because
//!    a systemd service is a session leader without a terminal, and opening a terminal
//!    without it would make the PTY the daemon's own controlling terminal), set the
//!    window size and `IUTF8` on the slave, and move the slave to a descriptor of 3 or
//!    more, so that `dup2` onto 0, 1 and 2 always makes a copy without close-on-exec.
//!    The master is an `OwnedFd` from its first line, and no descriptor number leaves
//!    this module.
//! 2. In the child (the `pre_exec` closure): `setsid`, `TIOCSCTTY` on the slave,
//!    `dup2` of the slave onto 0, 1 and 2, every standard signal back to its default
//!    action and none blocked, then close-on-exec on every descriptor from 3 up. The
//!    standard library has already changed to the spec's directory by then.
//!    Marking descriptors instead of closing them keeps the standard library's own
//!    close-on-exec pipe open until `execve`, so a failed `execve` still reaches the
//!    holder as an error; every descriptor but 0, 1 and 2 is closed by `execve`.
//! 3. In the holder again: drop the closure, which closes the holder's slave, so a read
//!    of the master ends with `EIO` once the child's side closes; adopt the child (see
//!    `child.rs`), keep a copy of the master for resizes and the foreground-group
//!    query, and hand the original to the caller.

use std::collections::HashMap;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::process::Stdio;
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use efr_holder::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyId, PtyInfo, Signal, SignalTarget, Size,
    SpawnSpec,
};
use rustix::fs::{Mode, OFlags};
use rustix::process::{Resource, getrlimit, ioctl_tiocsctty, kill_process_group, setsid};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use tokio::runtime::Handle;

use crate::child::{self, Child};
use crate::{PtyError, termios};

/// The lowest descriptor that is not a standard stream.
const FIRST_NON_STDIO_FD: RawFd = 3;

/// Where the `fcntl` fallback stops when the descriptor limit is unlimited or higher:
/// Linux hands out no descriptor at or above `fs.nr_open`, whose default is 2^20.
const FD_SCAN_CAP: u64 = 1 << 20;

/// A [`PtyHolder`] that opens PTYs and keeps their children in the calling process.
///
/// It is the holder at milestone 1, used by the daemon through its `local-pty` feature;
/// from milestone 5 `efr-ptyd` uses it behind the holder socket. Each child is a session
/// leader with its PTY as the controlling terminal, starts in the spec's directory with
/// exactly the spec's environment, and has only the PTY open, on descriptors 0, 1 and 2.
///
/// The holder keeps its own copy of every master until [`release`](PtyHolder::release),
/// so a child keeps its terminal while the caller reattaches, and gets `SIGHUP` only
/// once both copies are closed. A task on the caller's tokio runtime reaps each child as
/// it exits; the runtime must have IO enabled, as tokio's process support requires.
/// Dropping the holder closes its copies of the masters; the tasks still reap.
#[derive(Debug, Default)]
pub struct LocalPtyHolder {
    ptys: Mutex<HashMap<PtyId, Slot>>,
}

#[derive(Debug)]
enum Slot {
    /// A spawn with this id is under way. The slot holds the id, so a second spawn
    /// with it fails with `AlreadyExists`, but the PTY is not held yet: every other
    /// method answers `NotFound` for it.
    Spawning,
    Held(Held),
}

#[derive(Debug)]
struct Held {
    child: Child,
    size: Size,
    /// The holder's own copy of the master. Only ioctls go through it; the caller reads
    /// and writes its copy.
    master: OwnedFd,
}

/// A new PTY before its child starts.
struct Pty {
    master: OwnedFd,
    /// Never one of 0, 1 or 2, and close-on-exec.
    slave: OwnedFd,
}

impl LocalPtyHolder {
    /// A holder that holds no PTY yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PtyId, Slot>> {
        // No code panics while it holds the lock, and the table is valid after each
        // single update, so a poisoned lock still guards a consistent table.
        self.ptys.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims `pty_id` for a spawn, or fails with `AlreadyExists`.
    fn reserve(&self, pty_id: PtyId) -> Result<Reservation<'_>, HolderError> {
        let mut ptys = self.lock();
        if ptys.contains_key(&pty_id) {
            return Err(HolderError::AlreadyExists { pty_id });
        }
        ptys.insert(pty_id, Slot::Spawning);
        Ok(Reservation { holder: self, pty_id, filled: false })
    }
}

/// A claimed id. Dropping it unfilled, when a spawn fails, frees the id again.
struct Reservation<'a> {
    holder: &'a LocalPtyHolder,
    pty_id: PtyId,
    filled: bool,
}

impl Reservation<'_> {
    fn fill(mut self, held: Held) {
        self.holder.lock().insert(self.pty_id, Slot::Held(held));
        self.filled = true;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.filled {
            let mut ptys = self.holder.lock();
            if matches!(ptys.get(&self.pty_id), Some(Slot::Spawning)) {
                ptys.remove(&self.pty_id);
            }
        }
    }
}

fn held(ptys: &HashMap<PtyId, Slot>, pty_id: PtyId) -> Result<&Held, HolderError> {
    match ptys.get(&pty_id) {
        Some(Slot::Held(held)) => Ok(held),
        Some(Slot::Spawning) | None => Err(HolderError::NotFound { pty_id }),
    }
}

fn held_mut(ptys: &mut HashMap<PtyId, Slot>, pty_id: PtyId) -> Result<&mut Held, HolderError> {
    match ptys.get_mut(&pty_id) {
        Some(Slot::Held(held)) => Ok(held),
        Some(Slot::Spawning) | None => Err(HolderError::NotFound { pty_id }),
    }
}

#[async_trait]
impl PtyHolder for LocalPtyHolder {
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError> {
        spec.validate()?;
        let spawn_error = |source| HolderError::Spawn { program: spec.program.clone(), source };
        let runtime =
            Handle::try_current().map_err(|_| spawn_error(PtyError::NoRuntime.into_io()))?;
        let reservation = self.reserve(spec.pty_id)?;
        // NOTE: nothing below awaits, so the spawn cannot be cancelled half way; the
        // reservation still frees the id on every early return.
        let (held, handle) = start(&spec, &runtime).map_err(spawn_error)?;
        tracing::debug!(
            pty_id = %spec.pty_id,
            pid = handle.child_pid,
            program = %spec.program.display(),
            "started a child on a new PTY"
        );
        reservation.fill(held);
        Ok(handle)
    }

    async fn resize(&self, pty_id: PtyId, size: Size) -> Result<(), HolderError> {
        let mut ptys = self.lock();
        let held = held_mut(&mut ptys, pty_id)?;
        // Under the lock, so the size `list` reports is the size the kernel has.
        termios::set_size(&held.master, size)
            .map_err(|errno| HolderError::Resize { pty_id, source: errno.into() })?;
        held.size = size;
        Ok(())
    }

    async fn signal(
        &self,
        pty_id: PtyId,
        signal: Signal,
        target: SignalTarget,
    ) -> Result<(), HolderError> {
        let ptys = self.lock();
        let held = held(&ptys, pty_id)?;
        if !held.child.status().is_running() {
            return Err(HolderError::Exited { pty_id });
        }
        let signal_error = |source| HolderError::Signal { pty_id, signal, source };
        // Both contract enums may grow; a variant this build does not know is refused.
        let unsupported = || signal_error(io::Error::from(io::ErrorKind::Unsupported));
        let number = child::signal_number(signal).ok_or_else(unsupported)?;
        match target {
            SignalTarget::Child => match held.child.signal(number) {
                Ok(()) => Ok(()),
                // Reaped since the status check: the reaper is about to record it.
                Err(rustix::io::Errno::SRCH) => Err(HolderError::Exited { pty_id }),
                Err(errno) => Err(signal_error(errno.into())),
            },
            SignalTarget::ForegroundGroup => termios::foreground_group(&held.master)
                .and_then(|group| kill_process_group(group, number))
                .map_err(|errno| signal_error(errno.into())),
            _ => Err(unsupported()),
        }
    }

    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError> {
        let ptys = self.lock();
        let infos = ptys
            .iter()
            .filter_map(|(pty_id, slot)| match slot {
                Slot::Held(held) => Some(PtyInfo {
                    pty_id: *pty_id,
                    child_pid: held.child.id(),
                    size: held.size,
                    status: held.child.status(),
                }),
                Slot::Spawning => None,
            })
            .collect();
        Ok(infos)
    }

    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        let mut status = held(&self.lock(), pty_id)?.child.subscribe();
        // `wait_for` looks at the current value first, so a child reaped already answers
        // at once. An error means the sender is gone: the PTY was released.
        match status.wait_for(|status| !status.is_running()).await {
            Ok(status) => Ok(*status),
            Err(_) => Err(HolderError::NotFound { pty_id }),
        }
    }

    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        let released = {
            let mut ptys = self.lock();
            held(&ptys, pty_id)?;
            ptys.remove(&pty_id)
        };
        // Closing a master hangs up its terminal once no copy is left, which is kernel
        // work the table lock need not wait for.
        drop(released);
        Ok(())
    }
}

/// Opens a PTY of `spec.size`, starts the child on it and adopts the child.
fn start(spec: &SpawnSpec, runtime: &Handle) -> io::Result<(Held, PtyHandle)> {
    let Pty { master, slave } = open_pty(spec.size).map_err(PtyError::into_io)?;
    // Copied before the child starts, so that no failure after the start leaves a child
    // that nobody tracks.
    let holder_master = master.try_clone()?;
    let fd_limit = fd_limit();

    let mut command = efr_stdx::process::command(&spec.program, &spec.cwd);
    command
        .args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        // The closure replaces all three streams; inheriting them means the standard
        // library opens nothing for them first.
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // SAFETY: the closure runs in the child after `fork`, a copy of a process that may
    // run other threads, so it may call only async-signal-safe functions. `child_setup`
    // allocates nothing, takes no lock and touches no shared state: it makes the system
    // calls setsid, ioctl(TIOCSCTTY), dup2, signal, sigprocmask, close_range and fcntl,
    // through rustix (raw system calls on Linux) and libc, on the slave descriptor, on
    // descriptor and signal numbers and on a sigset_t on its own stack. The slave is
    // moved into the closure, so it is open whenever the closure runs; `fd_limit` is a
    // plain integer computed in the holder before the fork.
    unsafe {
        command.pre_exec(move || child_setup(slave.as_fd(), fd_limit));
    }
    let spawned = command.spawn()?;
    // The closure owns the holder's slave; dropping the command closes it.
    drop(command);
    let child = Child::adopt(spawned, runtime).map_err(PtyError::into_io)?;

    let handle = PtyHandle { master, child_pid: child.id(), pty_id: spec.pty_id };
    Ok((Held { child, size: spec.size, master: holder_master }, handle))
}

/// Opens a new PTY pair and sets up its terminal; see the module doc, step 1.
fn open_pty(size: Size) -> Result<Pty, PtyError> {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .map_err(|errno| PtyError::OpenMaster { source: errno.into() })?;
    grantpt(&master).map_err(|errno| PtyError::GrantSlave { source: errno.into() })?;
    unlockpt(&master).map_err(|errno| PtyError::UnlockSlave { source: errno.into() })?;
    let name = ptsname(&master, Vec::new())
        .map_err(|errno| PtyError::SlaveName { source: errno.into() })?;
    let open_slave_error = |errno: rustix::io::Errno| PtyError::OpenSlave {
        path: PathBuf::from(name.to_string_lossy().into_owned()),
        source: errno.into(),
    };
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_slave_error)?;
    let configure = |errno: rustix::io::Errno| PtyError::ConfigureTerminal { source: errno.into() };
    termios::set_size(&slave, size).map_err(configure)?;
    termios::enable_utf8(&slave).map_err(configure)?;
    // When the holder runs with a standard stream closed, the slave can land on 0, 1 or
    // 2, where `dup2` onto itself would keep close-on-exec and `execve` would close it.
    let slave =
        rustix::io::fcntl_dupfd_cloexec(&slave, FIRST_NON_STDIO_FD).map_err(open_slave_error)?;
    Ok(Pty { master, slave })
}

/// One more than the highest descriptor the process can have, for the `fcntl` fallback
/// in the child, which must not call `getrlimit` there.
fn fd_limit() -> RawFd {
    let limit = getrlimit(Resource::Nofile).current.unwrap_or(FD_SCAN_CAP).min(FD_SCAN_CAP);
    RawFd::try_from(limit).unwrap_or(RawFd::MAX)
}

/// The child's side of the spawn; see the module doc, step 2. Runs between `fork` and
/// `execve`, so everything in it must be async-signal-safe.
fn child_setup(slave: BorrowedFd<'_>, fd_limit: RawFd) -> io::Result<()> {
    // A new session without a controlling terminal, with the child as its leader and
    // the leader of a new process group.
    setsid()?;
    // The slave was opened with O_NOCTTY, so it is nobody's controlling terminal yet,
    // and a session leader without one may claim it.
    ioctl_tiocsctty(slave)?;
    rustix::stdio::dup2_stdin(slave)?;
    rustix::stdio::dup2_stdout(slave)?;
    rustix::stdio::dup2_stderr(slave)?;
    reset_signals()?;
    close_other_fds_on_exec(fd_limit);
    Ok(())
}

/// Puts every standard signal back to its default action and unblocks every signal.
///
/// `execve` resets caught signals but keeps ignored ones and the signal mask, and the
/// standard library resets only `SIGPIPE` (it keeps the mask of the thread that
/// spawns). A holder started with `SIGINT` or `SIGHUP` ignored (from `nohup`, or as a
/// background job of a shell without job control), or spawning from a thread that
/// blocks signals, would otherwise pass that on, and Ctrl+C would never reach the
/// hidden shell's commands. Resetting makes the child the same whichever process holds
/// it, as a terminal emulator's shell is.
fn reset_signals() -> io::Result<()> {
    // Linux numbers its standard signals 1 to 31 on every architecture.
    for signal in 1..32 {
        // SAFETY: signal(2) with SIG_DFL installs no handler, so no Rust code can run
        // from it, and POSIX lists signal() as async-signal-safe. For SIGKILL and
        // SIGSTOP, whose action cannot change, it fails with EINVAL, which is ignored.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
        }
    }
    let mut empty = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigemptyset writes a whole sigset_t through a pointer to storage of that
    // size and type, and sigprocmask reads it only after that write; the old mask is
    // not asked for, so its pointer is null. Both are async-signal-safe (POSIX lists
    // them), and after `fork` the child has a single thread, where sigprocmask is the
    // thread's mask.
    let unblocked = unsafe {
        libc::sigemptyset(empty.as_mut_ptr()) == 0
            && libc::sigprocmask(libc::SIG_SETMASK, empty.as_ptr(), ptr::null_mut()) == 0
    };
    if unblocked { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Marks every descriptor from 3 up close-on-exec, the slave's own copy and anything a
/// library opened without `O_CLOEXEC` included.
fn close_other_fds_on_exec(fd_limit: RawFd) {
    let first = FIRST_NON_STDIO_FD.unsigned_abs();
    // SAFETY: close_range takes three integers and touches no memory; with
    // CLOSE_RANGE_CLOEXEC it only sets a flag in this process's descriptor table. It is
    // one system call, which libc's `syscall` makes without allocating or locking, so
    // it is async-signal-safe.
    let marked =
        unsafe { libc::syscall(libc::SYS_close_range, first, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) };
    if marked == 0 {
        return;
    }
    // close_range is Linux 5.9 and its CLOEXEC flag 5.11; older kernels answer ENOSYS
    // or EINVAL. Walk the descriptors one by one instead.
    for fd in FIRST_NON_STDIO_FD..fd_limit {
        // SAFETY: fcntl(F_SETFD) takes integers and is async-signal-safe (POSIX lists
        // it). On a number that is not an open descriptor it fails with EBADF, which is
        // the expected answer for most numbers and is ignored; on an open one it sets
        // FD_CLOEXEC, the only descriptor flag, in this process alone.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

#[cfg(test)]
mod tests;
