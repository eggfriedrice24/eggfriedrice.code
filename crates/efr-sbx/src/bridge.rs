//! `efr-sbx bridge`: the bridge of a call with a proxy plan (the spec's section 8.2).
//!
//! The call's network namespace has loopback only. The bridge accepts TCP on
//! `127.0.0.1:3128` there and copies the bytes of each connection, both ways, to a new
//! connection to the call's proxy socket (`$XRT/efr-proxy.sock`), where efrd's proxy
//! decides. The child of the call cannot reach that socket itself: its Landlock domain
//! has no `RESOLVE_UNIX` rule for it.
//!
//! Two `std::process::Command` spawns start it, with the starter, the setup and the
//! copy of `helper.rs`, which the forwarder of a job shares:
//!
//! 1. The inner stage binds the listener before its own Landlock domain and runs the
//!    starter, `efr-sbx bridge --listen-fd 0 --socket P`, with the listener as stdin.
//! 2. The starter runs the server, `efr-sbx bridge --serve --listen-fd 0 --ready-fd 1
//!    --socket P`, with the listener as stdin and a pipe as stdout, and waits at most
//!    [`READY_WAIT`](crate::helper::READY_WAIT) for one byte `R` on the pipe. It exits 0
//!    on `R` and 1 otherwise; its exit leaves the server to bwrap's init, so the child's
//!    shell never sees it.
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
use std::io::{self, Write};
use std::net::TcpListener;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use efr_sandbox::{LandlockPolicy, SeccompProfile};
use rustix::io::Errno;

use crate::error::SbxError;
use crate::helper::{self, Confinement, Helper, Slot, Step, Take};
use crate::os;
use crate::self_test::socket_path;

/// The name of the server's process, which a job's stop spares (`job.rs`).
pub(crate) const BRIDGE_NAME: &str = "efr-bridge";

/// The most connections that the bridge copies at once; it closes one more at once.
pub(crate) const MAX_CONNECTIONS: usize = 64;

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

/// The error of a bridge with no listener on fd 0.
fn no_listener() -> SbxError {
    SbxError::os("take the bridge's listener", io::ErrorKind::InvalidInput.into())
}

/// The starter: runs the server and waits for its ready byte.
fn start(args: &BridgeArgs) -> Result<(), SbxError> {
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
    let Some(listen_fd) = args.listen_fd else { return Err(no_listener()) };
    helper::start(Helper::Bridge, argv, Some(listen_fd))
}

/// The bridge's confinement.
fn confinement(args: &BridgeArgs) -> Confinement {
    Confinement {
        helper: Helper::Bridge,
        landlock: LandlockPolicy::bridge(&args.socket),
        seccomp: SeccompProfile::bridge(),
        fail_setup: args.fail_setup,
    }
}

/// The path to connect to, with the descriptor of its dir when the path is too long
/// for a socket address. The bridge opens it before its own domain.
fn open_socket(socket: &Path) -> Result<(Option<OwnedFd>, PathBuf), SbxError> {
    socket_path(socket).map_err(|error| SbxError::io("find", socket, error))
}

/// The server: the setup, the ready byte, then the copies.
fn serve_main(args: &BridgeArgs) -> Result<(), SbxError> {
    let Some(listen_fd) = args.listen_fd else { return Err(no_listener()) };
    let take = Take { listen_fd: Some(listen_fd), ready_fd: args.ready_fd };
    let (streams, (_dir, socket)) =
        helper::setup(&confinement(args), Some(take), || open_socket(&args.socket))?;
    let Some(listener) = streams.listener else { return Err(no_listener()) };
    let Some(ready) = streams.ready else {
        return Err(SbxError::os("take the ready pipe", io::ErrorKind::InvalidInput.into()));
    };
    helper::report_ready(ready)?;
    serve(&TcpListener::from(listener), &socket);
    Ok(())
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
        let Some(slot) = Slot::take(&open, MAX_CONNECTIONS) else {
            drop(tcp);
            continue;
        };
        let socket = socket.to_path_buf();
        let _ = helper::spawn_copy(Helper::Bridge, move || {
            if let Ok(unix) = UnixStream::connect(&socket) {
                helper::copy_both(Helper::Bridge, tcp, unix);
            }
            drop(slot);
        });
    }
}

/// `--check-dumpable`: the server's setup with no listener, then the checks. Prints one
/// JSON line per check, as the self-test does.
fn check(args: &BridgeArgs) -> Result<(), SbxError> {
    let setup = helper::setup(&confinement(args), None, || Ok(()))?;
    let mut lines = vec![helper::dumpable_line()];
    if let Some(dir) = &args.write {
        lines.push(helper::write_line(dir, ".efr-bridge-write"));
    }
    if let Some(reach) = &args.reach {
        let done = socket_path(reach).and_then(|(_dir, path)| UnixStream::connect(path));
        let ok = done
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(Errno::ACCESS.raw_os_error()));
        lines.push(helper::line("reach", ok, format!("{:?}", done.map(drop))));
    }
    let inet = rustix::net::socket(
        rustix::net::AddressFamily::INET,
        rustix::net::SocketType::STREAM,
        None,
    );
    lines.push(helper::line(
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
    lines.push(helper::line("bind", bind == Err(Errno::PERM), format!("{bind:?}")));
    lines.push(helper::exec_line());
    drop(setup);
    if helper::print_lines(&lines) {
        Ok(())
    } else {
        Err(SbxError::HelperStart {
            helper: Helper::Bridge,
            problem: "a check of the bridge's own sandbox failed",
        })
    }
}

#[cfg(test)]
mod tests;
