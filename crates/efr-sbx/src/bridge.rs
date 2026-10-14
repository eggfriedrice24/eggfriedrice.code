//! `efr-sbx bridge`: the bridge of a call with a proxy plan (the spec's section 8.2).
//!
//! The call's network namespace has loopback only. The bridge accepts TCP on
//! `127.0.0.1:3128` there and copies the bytes of each connection, both ways, to a new
//! connection to the call's proxy socket (`$XRT/efr-proxy.sock`), where efrd's proxy
//! decides. The child of the call cannot reach that socket itself: its Landlock domain
//! has no `RESOLVE_UNIX` rule for it.
//!
//! Two `std::process::Command` spawns start it, so it needs no code outside safe Rust
//! (ADR 0007 keeps that code in `fds.rs`):
//!
//! 1. The inner stage binds the listener before its own Landlock domain and runs the
//!    starter, `efr-sbx bridge --listen-fd 0 --socket P`, with the listener as stdin.
//! 2. The starter runs the server, `efr-sbx bridge --serve --listen-fd 0 --ready-fd 1
//!    --socket P`, with the listener as stdin and a pipe as stdout, and waits at most
//!    [`READY_WAIT`] for one byte `R` on the pipe. It exits 0 on `R` and 1 otherwise;
//!    its exit leaves the server to bwrap's init, so the child's shell never sees it.
//! 3. The server closes every descriptor from 3 up, makes a session of its own, names
//!    itself `efr-bridge`, makes itself not dumpable, and applies its own Landlock
//!    domain (`LandlockPolicy::bridge`) and seccomp profile (`SeccompProfile::bridge`).
//!    Only then does it write `R`. A failed step ends it before `R`, so it never serves
//!    unconfined. Then it copies bytes with two threads per connection, at most
//!    [`MAX_CONNECTIONS`] connections at once.
//!
//! `--check-dumpable` runs the server's setup with no listener and checks it: the
//! process is not dumpable, and with `--write` and `--reach` a write and a connect that
//! the call itself may make fail in the bridge's domain. `--fail-setup STEP` (the
//! starter passes it on to the server) ends the server at that step, for the tests; the
//! inner stage never passes it, and a failed setup only ends the bridge.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use efr_sandbox::{LandlockPolicy, SeccompProfile};
use rustix::io::Errno;
use rustix::process::DumpableBehavior;

use crate::error::SbxError;
use crate::self_test::{CheckLine, socket_path};
use crate::{fds, landlock, os, seccomp};

/// How long the starter waits for the server's ready byte.
pub(crate) const READY_WAIT: Duration = Duration::from_secs(1);

/// The byte that the server writes once its own sandbox is in place.
const READY: u8 = b'R';

/// The name of the server's process, which a job's stop spares (`job.rs`).
pub(crate) const BRIDGE_NAME: &str = "efr-bridge";

/// [`BRIDGE_NAME`] for `PR_SET_NAME`.
const BRIDGE_CNAME: &std::ffi::CStr = c"efr-bridge";

/// The most connections that the bridge copies at once; it closes one more at once.
pub(crate) const MAX_CONNECTIONS: usize = 64;

/// The stack of each copy thread: the copy needs only a small buffer.
const COPY_STACK: usize = 128 * 1024;

/// A step of the server's setup, for `--fail-setup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Step {
    /// Close the descriptors from 3 up.
    Close,
    /// Make a session of its own.
    Session,
    /// Name the process `efr-bridge`.
    Name,
    /// Make the process not dumpable.
    Dumpable,
    /// Apply the bridge's Landlock domain.
    Landlock,
    /// Install the bridge's seccomp profile.
    Seccomp,
}

impl Step {
    fn word(self) -> &'static str {
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

/// The arguments of `efr-sbx bridge`.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct BridgeArgs {
    /// The listening descriptor: 0, the TCP listener on `127.0.0.1:3128`.
    #[arg(long)]
    pub(crate) listen_fd: Option<i32>,
    /// The call's proxy socket inside, `$XRT/efr-proxy.sock`.
    #[arg(long)]
    pub(crate) socket: PathBuf,
    /// Run the server: the setup, the ready byte, then the copies.
    #[arg(long)]
    pub(crate) serve: bool,
    /// The server's ready pipe: 1.
    #[arg(long)]
    pub(crate) ready_fd: Option<i32>,
    /// Run the server's setup with no listener; exit 0 when the process is not
    /// dumpable and every named check is refused.
    #[arg(long)]
    pub(crate) check_dumpable: bool,
    /// With `--check-dumpable`: a directory that the call may write; a new file there
    /// must fail with EACCES.
    #[arg(long, requires = "check_dumpable")]
    pub(crate) write: Option<PathBuf>,
    /// With `--check-dumpable`: a socket that the call may reach; a connect must fail
    /// with EACCES.
    #[arg(long, requires = "check_dumpable")]
    pub(crate) reach: Option<PathBuf>,
    /// End the server's setup before this step, for the tests.
    #[arg(long, value_enum)]
    pub(crate) fail_setup: Option<Step>,
}

/// Runs the starter, the server or the check.
pub(crate) fn main(args: &BridgeArgs) -> ExitCode {
    let done = if args.check_dumpable {
        check(args)
    } else if args.serve {
        serve_main(args)
    } else {
        start(args)
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // NOTE: the inner stage gives the starter /dev/null as stderr; the text is
            // for a person who runs it by hand.
            let _ = writeln!(io::stderr(), "efr-sbx bridge: {}", error.chain());
            ExitCode::FAILURE
        }
    }
}

/// The descriptor `fd` of the standard streams as one of this process's own: 0 for the
/// listener, 1 for the ready pipe. The server closes every descriptor from 3 up first,
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

/// The starter: runs the server and waits for its ready byte.
fn start(args: &BridgeArgs) -> Result<(), SbxError> {
    // NOTE: the inner stage's records pipe has no close-on-exec flag; the bridge must
    // never hold it, or the launcher would wait for its end.
    fds::close_from(3).map_err(|error| SbxError::os("close the inherited descriptors", error))?;
    let listener = take_stdio(args.listen_fd, 0, "take the bridge's listener")?;
    let (ready_read, ready_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|error| SbxError::os("make the bridge's ready pipe", error.into()))?;
    let mut argv: Vec<OsString> = vec![
        "bridge".into(),
        "--serve".into(),
        "--listen-fd".into(),
        "0".into(),
        "--ready-fd".into(),
        "1".into(),
        "--socket".into(),
        args.socket.clone().into_os_string(),
    ];
    if let Some(step) = args.fail_setup {
        argv.extend(["--fail-setup".into(), step.word().into()]);
    }
    let mut command = os::command(SELF_EXE);
    command
        .args(&argv)
        .env_clear()
        .stdin(Stdio::from(listener))
        .stdout(Stdio::from(ready_write))
        .stderr(Stdio::null());
    let spawned = command.spawn();
    // The starter's copies of the listener and of the pipe's write end close here, so
    // the read below sees the end of the pipe when the server exits.
    drop(command);
    // NOTE: the server is never waited for: the starter's exit leaves it to bwrap's
    // init, which reaps it.
    let _server = spawned.map_err(|source| SbxError::Spawn { program: SELF_EXE.into(), source })?;
    match read_ready(ready_read, READY_WAIT) {
        Some(READY) => Ok(()),
        Some(_) => Err(SbxError::BridgeStart { problem: "the server sent a wrong ready byte" }),
        None => Err(SbxError::BridgeStart { problem: "the server did not report that it runs" }),
    }
}

/// The program that runs the server: this launcher, by the kernel's own link, so no
/// path in the call can stand in for it.
const SELF_EXE: &str = "/proc/self/exe";

/// One byte from `pipe`, or `None` at its end or after `wait`.
fn read_ready(pipe: OwnedFd, wait: Duration) -> Option<u8> {
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

/// The server: the setup, the ready byte, then the copies.
fn serve_main(args: &BridgeArgs) -> Result<(), SbxError> {
    let setup = setup(args, true)?;
    let Some(listener) = setup.listener else {
        return Err(SbxError::os("take the bridge's listener", io::ErrorKind::InvalidInput.into()));
    };
    let Some(ready) = setup.ready else {
        return Err(SbxError::os("take the ready pipe", io::ErrorKind::InvalidInput.into()));
    };
    let mut ready = File::from(ready);
    ready
        .write_all(&[READY])
        .map_err(|error| SbxError::os("report that the bridge runs", error))?;
    drop(ready);
    serve(&TcpListener::from(listener), &setup.socket);
    Ok(())
}

/// What the setup leaves: the listener and the ready pipe of a server, and the path to
/// connect to, with the descriptor of its dir when the path is too long for a socket
/// address.
struct Setup {
    listener: Option<OwnedFd>,
    ready: Option<OwnedFd>,
    socket: PathBuf,
    _dir: Option<OwnedFd>,
}

/// The server's setup, in order. `serve` takes the listener and the ready pipe and
/// puts `/dev/null` on fd 0 and 1; the check keeps its stdout for its lines.
fn setup(args: &BridgeArgs, serve: bool) -> Result<Setup, SbxError> {
    let fail = |step: Step| -> Result<(), SbxError> {
        match args.fail_setup {
            Some(failed) if failed == step => {
                Err(SbxError::BridgeSetupStopped { step: step.word() })
            }
            _ => Ok(()),
        }
    };
    fail(Step::Close)?;
    fds::close_from(3).map_err(|error| SbxError::os("close the inherited descriptors", error))?;
    let (listener, ready) = if serve {
        let listener = take_stdio(args.listen_fd, 0, "take the bridge's listener")?;
        let ready = take_stdio(args.ready_fd, 1, "take the ready pipe")?;
        let null = File::options()
            .read(true)
            .write(true)
            .open("/dev/null")
            .map_err(|error| SbxError::io("open", "/dev/null", error))?;
        rustix::stdio::dup2_stdin(&null)
            .and_then(|()| rustix::stdio::dup2_stdout(&null))
            .map_err(|error| SbxError::os("put /dev/null on stdin and stdout", error.into()))?;
        (Some(listener), Some(ready))
    } else {
        (None, None)
    };
    fail(Step::Session)?;
    // NOTE: a session of its own, so the terminal's Ctrl+C and its hangup never reach
    // the bridge. The starter's exit then leaves it to bwrap's init.
    rustix::process::setsid()
        .map_err(|error| SbxError::os("make the bridge's session", error.into()))?;
    fail(Step::Name)?;
    rustix::thread::set_name(BRIDGE_CNAME)
        .map_err(|error| SbxError::os("name the bridge", error.into()))?;
    fail(Step::Dumpable)?;
    // NOTE: not dumpable: the call's child cannot ptrace the bridge, read its memory, its
    // environment or its descriptors, and its /proc files belong to root.
    rustix::process::set_dumpable_behavior(DumpableBehavior::NotDumpable)
        .map_err(|error| SbxError::os("make the bridge not dumpable", error.into()))?;
    // A path that is too long for a socket address goes through its dir's descriptor,
    // which the bridge opens before its own domain.
    let (dir, socket) =
        socket_path(&args.socket).map_err(|error| SbxError::io("find", &args.socket, error))?;
    fail(Step::Landlock)?;
    landlock::restrict(&LandlockPolicy::bridge(&args.socket), &[])?;
    fail(Step::Seccomp)?;
    seccomp::install(&SeccompProfile::bridge())?;
    Ok(Setup { listener, ready, socket, _dir: dir })
}

/// Accepts connections and copies each one to a new connection to `socket`, at most
/// [`MAX_CONNECTIONS`] at once.
fn serve(listener: &TcpListener, socket: &Path) {
    let open = Arc::new(AtomicUsize::new(0));
    loop {
        let tcp = match listener.accept() {
            Ok((tcp, _)) => tcp,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                // NOTE: out of descriptors or memory for a moment; the next accept
                // tries again, without a busy loop.
                os::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            drop(tcp);
            continue;
        }
        let slot = Slot(Arc::clone(&open));
        let socket = socket.to_path_buf();
        let _ = std::thread::Builder::new()
            .name("efr-bridge-copy".to_owned())
            .stack_size(COPY_STACK)
            .spawn(move || {
                copy_both(tcp, &socket);
                drop(slot);
            });
    }
}

/// One open connection; its drop frees the place.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Copies `tcp` to a new connection to `socket` and back until both ends are done.
fn copy_both(tcp: TcpStream, socket: &Path) {
    let Ok(unix) = UnixStream::connect(socket) else { return };
    let (Ok(tcp_back), Ok(unix_back)) = (tcp.try_clone(), unix.try_clone()) else { return };
    let back = std::thread::Builder::new()
        .name("efr-bridge-copy".to_owned())
        .stack_size(COPY_STACK)
        .spawn(move || copy(unix_back, tcp_back));
    copy(tcp, unix);
    if let Ok(back) = back {
        let _ = back.join();
    }
}

/// The half-closes of one direction of a copy.
trait HalfClose {
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
fn copy<R, W>(mut from: R, mut to: W)
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

/// `--check-dumpable`: the server's setup with no listener, then the checks. Prints one
/// JSON line per check, as the self-test does.
fn check(args: &BridgeArgs) -> Result<(), SbxError> {
    let setup = setup(args, false)?;
    let mut lines = vec![dumpable_line()];
    if let Some(dir) = &args.write {
        let path = dir.join(".efr-bridge-write");
        let done = File::create(&path);
        let ok = done
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(Errno::ACCESS.raw_os_error()));
        lines.push(line("write", ok, format!("{:?}", done.map(drop))));
    }
    if let Some(reach) = &args.reach {
        let done = socket_path(reach).and_then(|(_dir, path)| UnixStream::connect(path));
        let ok = done
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(Errno::ACCESS.raw_os_error()));
        lines.push(line("reach", ok, format!("{:?}", done.map(drop))));
    }
    let inet = rustix::net::socket(
        rustix::net::AddressFamily::INET,
        rustix::net::SocketType::STREAM,
        None,
    );
    lines.push(line(
        "inet",
        matches!(inet, Err(Errno::AFNOSUPPORT)),
        format!("{:?}", inet.map(drop)),
    ));
    let bind = rustix::net::socket(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        None,
    )
    .and_then(|socket| rustix::net::bind(&socket, &rustix::net::SocketAddrUnix::new_unnamed()));
    lines.push(line("bind", bind == Err(Errno::PERM), format!("{bind:?}")));
    let exec = os::command(SELF_EXE).arg("--version").stdin(Stdio::null()).output();
    let refused =
        exec.as_ref().is_err_and(|error| error.raw_os_error() == Some(Errno::PERM.raw_os_error()));
    lines.push(line("exec", refused, format!("{:?}", exec.map(|out| out.status))));
    drop(setup);
    let mut out = io::stdout().lock();
    let mut all = true;
    for check in &lines {
        all &= check.ok;
        let text = serde_json::to_string(check).unwrap_or_default();
        let _ = writeln!(out, "{text}");
    }
    if all {
        Ok(())
    } else {
        Err(SbxError::BridgeStart { problem: "a check of the bridge's own sandbox failed" })
    }
}

fn dumpable_line() -> CheckLine {
    let behavior = rustix::process::dumpable_behavior();
    line("dumpable", behavior == Ok(DumpableBehavior::NotDumpable), format!("{behavior:?}"))
}

fn line(name: &str, ok: bool, detail: String) -> CheckLine {
    CheckLine { name: name.to_owned(), ok, detail }
}

#[cfg(test)]
mod tests;
