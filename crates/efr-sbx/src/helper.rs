//! The start, the setup and the copy that the confined helpers inside a sandbox share:
//! the bridge of a proxy call (`bridge.rs`) and the forwarder of a job (`forward.rs`).
//!
//! Two `std::process::Command` spawns start a helper, so it needs no code outside safe
//! Rust (ADR 0007 keeps that code in `fds.rs`):
//!
//! 1. The inner stage runs the starter, `efr-sbx <helper> ...`.
//! 2. The starter closes every descriptor from 3 up, runs the server, `efr-sbx <helper>
//!    --serve --ready-fd 1 ...`, with a pipe as stdout, and waits at most
//!    [`READY_WAIT`] for one byte [`READY`] on the pipe. It exits 0 on `R` and 1
//!    otherwise; its exit leaves the server to bwrap's init, so the child's shell never
//!    sees it.
//! 3. The server runs [`setup`]: it closes every descriptor from 3 up, makes a session
//!    of its own, names itself, opens what it needs before its domain, makes itself not
//!    dumpable, and applies its own Landlock domain and seccomp profile. Only then does
//!    it write `R`. A failed step ends it before `R`, so it never serves unconfined.
//!
//! `--fail-setup STEP` (the starter passes it on to the server) ends the server at that
//! step, for the tests; the inner stage never passes it.

use std::ffi::{CStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use efr_sandbox::{LandlockPolicy, SeccompProfile};
use rustix::io::Errno;
use rustix::process::DumpableBehavior;

use crate::error::SbxError;
use crate::self_test::CheckLine;
use crate::{fds, landlock, os, seccomp};

/// How long the starter waits for the server's ready byte.
pub(crate) const READY_WAIT: Duration = Duration::from_secs(1);

/// The byte that the server writes once its own sandbox is in place.
pub(crate) const READY: u8 = b'R';

/// The stack of each copy thread: the copy needs only a small buffer.
pub(crate) const COPY_STACK: usize = 128 * 1024;

/// The program that runs the server: this launcher, by the kernel's own link, so no
/// path in the call can stand in for it.
pub(crate) const SELF_EXE: &str = "/proc/self/exe";

/// A confined helper inside a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Helper {
    /// The bridge of a proxy call, `efr-sbx bridge`.
    Bridge,
    /// The forwarder of a job with port forwards, `efr-sbx forward`.
    Forwarder,
}

impl Helper {
    /// The name of the server's process, which a job's stop spares (`job.rs`).
    pub(crate) fn name(self) -> &'static CStr {
        match self {
            Helper::Bridge => c"efr-bridge",
            Helper::Forwarder => c"efr-forward",
        }
    }

    /// The name of its copy threads.
    fn copy_thread(self) -> &'static str {
        match self {
            Helper::Bridge => "efr-bridge-copy",
            Helper::Forwarder => "efr-fwd-copy",
        }
    }
}

impl fmt::Display for Helper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Helper::Bridge => "network bridge",
            Helper::Forwarder => "port forwarder",
        })
    }
}

/// A step of the server's setup, for `--fail-setup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Step {
    /// Close the descriptors from 3 up.
    Close,
    /// Make a session of its own.
    Session,
    /// Name the process.
    Name,
    /// Make the process not dumpable.
    Dumpable,
    /// Apply the helper's Landlock domain.
    Landlock,
    /// Install the helper's seccomp profile.
    Seccomp,
}

impl Step {
    /// Every step, in the order of the setup.
    #[cfg(test)]
    pub(crate) const ALL: [Step; 6] =
        [Step::Close, Step::Session, Step::Name, Step::Dumpable, Step::Landlock, Step::Seccomp];

    /// The word of `--fail-setup`.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Step::Close => "close",
            Step::Session => "session",
            Step::Name => "name",
            Step::Dumpable => "dumpable",
            Step::Landlock => "landlock",
            Step::Seccomp => "seccomp",
        }
    }
}

/// The descriptor `fd` of the standard streams as one of this process's own: 0 for a
/// listener, 1 for the ready pipe. The caller closed every descriptor from 3 up first,
/// so no other number can reach it.
fn take_stdio(fd: Option<i32>, expected: i32, what: &'static str) -> Result<OwnedFd, SbxError> {
    let refused = || SbxError::os(what, io::ErrorKind::InvalidInput.into());
    let copy = match fd {
        Some(0) if expected == 0 => io::stdin().as_fd().try_clone_to_owned(),
        Some(1) if expected == 1 => io::stdout().as_fd().try_clone_to_owned(),
        _ => return Err(refused()),
    };
    copy.map_err(|error| SbxError::os(what, error))
}

/// The starter: closes every descriptor from 3 up, runs the server with `argv` (which
/// names `--ready-fd 1`), with the listener on fd `listen_fd` (0) as its stdin when one
/// is named, else `/dev/null`, and waits for its ready byte.
pub(crate) fn start(
    helper: Helper,
    argv: Vec<OsString>,
    listen_fd: Option<i32>,
) -> Result<(), SbxError> {
    // NOTE: the inner stage's records pipe has no close-on-exec flag; a helper must
    // never hold it, or the launcher would wait for its end.
    fds::close_from(3).map_err(|error| SbxError::os("close the inherited descriptors", error))?;
    let stdin = match listen_fd {
        Some(_) => Stdio::from(take_stdio(listen_fd, 0, "take the helper's listener")?),
        None => Stdio::null(),
    };
    let (ready_read, ready_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|error| SbxError::os("make the helper's ready pipe", error.into()))?;
    let mut command = os::command(SELF_EXE);
    command
        .args(&argv)
        .env_clear()
        .stdin(stdin)
        .stdout(Stdio::from(ready_write))
        .stderr(Stdio::null());
    let spawned = command.spawn();
    // The starter's copies of the listener and of the pipe's write end close here, so
    // the read below sees the end of the pipe when the server exits.
    drop(command);
    // NOTE: a server that runs is never waited for: the starter's exit leaves it to
    // bwrap's init, which reaps it.
    let mut server =
        spawned.map_err(|source| SbxError::Spawn { program: SELF_EXE.into(), source })?;
    let problem = match read_ready(ready_read, READY_WAIT) {
        Some(READY) => return Ok(()),
        Some(_) => "the server sent a wrong ready byte",
        None => "the server did not report that it runs",
    };
    // A server that is late or wrong ends here, so none runs on after its starter
    // failed, also one that is still in its setup.
    let _ = server.kill();
    let _ = server.wait();
    Err(SbxError::HelperStart { helper, problem })
}

/// One byte from `pipe`, or `None` at its end or after `wait`.
pub(crate) fn read_ready(pipe: OwnedFd, wait: Duration) -> Option<u8> {
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new().name("efr-sbx-ready".to_owned()).spawn(move || {
        let mut byte = [0_u8; 1];
        let read = File::from(pipe).read(&mut byte);
        let _ = send.send(matches!(read, Ok(1)).then_some(byte[0]));
    });
    if reader.is_err() {
        return None;
    }
    receive.recv_timeout(wait).ok().flatten()
}

/// Writes the ready byte on `ready` and closes it.
pub(crate) fn report_ready(ready: OwnedFd) -> Result<(), SbxError> {
    let mut ready = File::from(ready);
    ready.write_all(&[READY]).map_err(|error| SbxError::os("report that the helper runs", error))
}

/// One helper's confinement.
#[derive(Debug, Clone)]
pub(crate) struct Confinement {
    /// Which helper.
    pub(crate) helper: Helper,
    /// Its own Landlock domain.
    pub(crate) landlock: LandlockPolicy,
    /// Its own seccomp profile.
    pub(crate) seccomp: SeccompProfile,
    /// The step before which the setup ends, for the tests.
    pub(crate) fail_setup: Option<Step>,
}

/// The standard streams that a server takes in its setup: the listener on fd 0 when
/// `listen_fd` names it, and the ready pipe on fd 1. The setup then puts `/dev/null` on
/// fd 0 and 1.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Take {
    /// The listener's descriptor, 0, or `None` for a server with no listener.
    pub(crate) listen_fd: Option<i32>,
    /// The ready pipe's descriptor, 1.
    pub(crate) ready_fd: Option<i32>,
}

/// What a server's setup took of its standard streams.
#[derive(Debug, Default)]
pub(crate) struct Streams {
    /// The listener, when the server has one.
    pub(crate) listener: Option<OwnedFd>,
    /// The ready pipe.
    pub(crate) ready: Option<OwnedFd>,
}

/// A server's setup, in order: close the descriptors from 3 up, take the standard
/// streams of `take` (a check takes none and keeps its stdout for its lines), make a
/// session, set the name, run `open` (what the server opens before its domain),
/// become not dumpable, apply the Landlock domain, install the seccomp profile.
pub(crate) fn setup<T>(
    confinement: &Confinement,
    take: Option<Take>,
    open: impl FnOnce() -> Result<T, SbxError>,
) -> Result<(Streams, T), SbxError> {
    let helper = confinement.helper;
    let fail = |step: Step| -> Result<(), SbxError> {
        match confinement.fail_setup {
            Some(failed) if failed == step => {
                Err(SbxError::SetupStopped { helper, step: step.word() })
            }
            _ => Ok(()),
        }
    };
    fail(Step::Close)?;
    fds::close_from(3).map_err(|error| SbxError::os("close the inherited descriptors", error))?;
    let streams = match take {
        Some(take) => {
            let listener = match take.listen_fd {
                Some(_) => Some(take_stdio(take.listen_fd, 0, "take the helper's listener")?),
                None => None,
            };
            let ready = take_stdio(take.ready_fd, 1, "take the ready pipe")?;
            let null = File::options()
                .read(true)
                .write(true)
                .open("/dev/null")
                .map_err(|error| SbxError::io("open", "/dev/null", error))?;
            rustix::stdio::dup2_stdin(&null)
                .and_then(|()| rustix::stdio::dup2_stdout(&null))
                .map_err(|error| SbxError::os("put /dev/null on stdin and stdout", error.into()))?;
            Streams { listener, ready: Some(ready) }
        }
        None => Streams::default(),
    };
    fail(Step::Session)?;
    // NOTE: a session of its own, so the terminal's Ctrl+C and its hangup never reach
    // the helper. The starter's exit then leaves it to bwrap's init.
    rustix::process::setsid()
        .map_err(|error| SbxError::os("make the helper's session", error.into()))?;
    fail(Step::Name)?;
    rustix::thread::set_name(helper.name())
        .map_err(|error| SbxError::os("name the helper", error.into()))?;
    let opened = open()?;
    fail(Step::Dumpable)?;
    // NOTE: not dumpable: the call's child cannot ptrace the helper, read its memory,
    // its environment or its descriptors, and its /proc files belong to root.
    rustix::process::set_dumpable_behavior(DumpableBehavior::NotDumpable)
        .map_err(|error| SbxError::os("make the helper not dumpable", error.into()))?;
    fail(Step::Landlock)?;
    landlock::restrict(&confinement.landlock, &[])?;
    fail(Step::Seccomp)?;
    seccomp::install(&confinement.seccomp)?;
    Ok((streams, opened))
}

/// One open connection of a helper; its drop frees the place.
#[derive(Debug)]
pub(crate) struct Slot(Arc<AtomicUsize>);

impl Slot {
    /// A place among at most `max` open connections that `open` counts, or `None` when
    /// all are taken.
    pub(crate) fn take(open: &Arc<AtomicUsize>, max: usize) -> Option<Slot> {
        if open.fetch_add(1, Ordering::SeqCst) >= max {
            open.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(Arc::clone(open)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs `work` on a new thread of `helper` with the small stack of a copy.
pub(crate) fn spawn_copy(
    helper: Helper,
    work: impl FnOnce() + Send + 'static,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(helper.copy_thread().to_owned())
        .stack_size(COPY_STACK)
        .spawn(work)
}

/// Copies `tcp` to `unix` and back until both ends are done, the way back on a thread
/// of its own.
pub(crate) fn copy_both(helper: Helper, tcp: TcpStream, unix: UnixStream) {
    let (Ok(tcp_back), Ok(unix_back)) = (tcp.try_clone(), unix.try_clone()) else { return };
    let back = spawn_copy(helper, move || copy(unix_back, tcp_back));
    copy(tcp, unix);
    if let Ok(back) = back {
        let _ = back.join();
    }
}

/// The half-closes of one direction of a copy.
pub(crate) trait HalfClose {
    fn half_close(&self, how: Shutdown);
}

impl HalfClose for TcpStream {
    fn half_close(&self, how: Shutdown) {
        let _ = self.shutdown(how);
    }
}

impl HalfClose for UnixStream {
    fn half_close(&self, how: Shutdown) {
        let _ = self.shutdown(how);
    }
}

/// Copies `from` to `to`. At the end of `from`, `to` gets the end too; on an error,
/// both connections end both ways, so the other direction's copy ends as well.
pub(crate) fn copy<R, W>(mut from: R, mut to: W)
where
    R: Read + HalfClose,
    W: Write + HalfClose,
{
    match io::copy(&mut from, &mut to) {
        Ok(_) => to.half_close(Shutdown::Write),
        Err(_) => {
            from.half_close(Shutdown::Both);
            to.half_close(Shutdown::Both);
        }
    }
}

/// A check line of a helper's `--check` mode.
pub(crate) fn line(name: &str, ok: bool, detail: String) -> CheckLine {
    CheckLine { name: name.to_owned(), ok, detail }
}

/// The check that the process is not dumpable.
pub(crate) fn dumpable_line() -> CheckLine {
    let behavior = rustix::process::dumpable_behavior();
    line("dumpable", behavior == Ok(DumpableBehavior::NotDumpable), format!("{behavior:?}"))
}

/// The check that a new file `name` in `dir`, which the call may write, fails with
/// EACCES in the helper's domain.
pub(crate) fn write_line(dir: &Path, name: &str) -> CheckLine {
    let done = File::create(dir.join(name));
    let ok = done
        .as_ref()
        .is_err_and(|error| error.raw_os_error() == Some(Errno::ACCESS.raw_os_error()));
    line("write", ok, format!("{:?}", done.map(drop)))
}

/// The check that a program does not start: the helper's seccomp profile refuses
/// `execve` with EPERM.
pub(crate) fn exec_line() -> CheckLine {
    // NOTE: the standard streams stay this process's own: the helper's domain cannot
    // open /dev/null, which would refuse the spawn before its exec.
    let exec = os::command(SELF_EXE)
        .arg("--version")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .and_then(|mut child| child.wait());
    let refused =
        exec.as_ref().is_err_and(|error| error.raw_os_error() == Some(Errno::PERM.raw_os_error()));
    line("exec", refused, format!("{exec:?}"))
}

/// Prints `lines` as JSON lines on stdout and says whether all passed.
pub(crate) fn print_lines(lines: &[CheckLine]) -> bool {
    let mut out = io::stdout().lock();
    let mut all = true;
    for check in lines {
        all &= check.ok;
        let text = serde_json::to_string(check).unwrap_or_default();
        let _ = writeln!(out, "{text}");
    }
    all
}

#[cfg(test)]
mod tests;
