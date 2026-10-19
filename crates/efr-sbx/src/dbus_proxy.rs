//! `efr-sbx dbus-proxy`: the wrapper that efrd runs `xdg-dbus-proxy` through (phase E).
//!
//! efrd starts one proxy per filtered bus per call, with the trusted program's checked
//! path, `/dev/null` as stdin, the write end of its sync pipe as stdout and a pipe as
//! stderr:
//!
//! ```text
//! efr-sbx dbus-proxy --dir $CALL/bus --upstream <bus socket>... -- <xdg-dbus-proxy> --fd=3 ...
//! ```
//!
//! The wrapper confines the proxy and runs it, in this order:
//!
//! 1. It closes every descriptor from 3 up, then moves its stdout, the sync pipe, to
//!    fd 3 with [`fds::dup_to`], where the proxy's `--fd=3` finds it, and points fd 1
//!    at its stderr: `xdg-dbus-proxy` 0.1.9 prints its `--log` lines on stdout, and
//!    efrd reads them, with the proxy's own errors, from the one stderr pipe. efrd
//!    cannot place a pipe at fd 3 itself without `pre_exec`.
//! 2. It changes to `--dir`, the call's bus dir, so the proxy binds its sockets by
//!    relative names (`system.sock`), and a deep call dir never meets the 107 bytes of
//!    a socket address.
//! 3. It sets `RLIMIT_NOFILE` to [`MAX_FILES`] (soft and hard): enough for the
//!    clients of one call, two descriptors each, never a flood of upstream
//!    connections.
//! 4. It sets `no_new_privs`, applies `LandlockPolicy::bus_proxy` (read and run only
//!    the program and the library dirs, connect only to each `--upstream`, make and
//!    remove sockets only in `--dir`, no TCP, abstract sockets and signals scoped),
//!    installs `SeccompProfile::bus_proxy` (only `AF_UNIX` sockets, no `ptrace`), and
//!    runs the program.
//!
//! Any failure before the exec exits [`SETUP_FAILURE_STATUS`] with one line on stderr,
//! and the proxy never runs unconfined. No step needs code outside safe Rust but
//! [`fds`], which ADR 0007 already allows. The domain limits files and network only:
//! the proxy keeps an unfiltered connection to each bus for each client, so a bug in its
//! parser still reaches the user's rights on that bus.

use std::convert::Infallible;
use std::ffi::OsString;
use std::io::{self, Write};
use std::os::fd::{AsFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::ExitCode;

use efr_sandbox::{LandlockPolicy, SETUP_FAILURE_STATUS, SeccompProfile};
use rustix::process::{Resource, Rlimit};

use crate::error::SbxError;
use crate::{fds, landlock, os, seccomp};

/// The proxy's limit of open descriptors: the sync pipe, the standard streams, GLib's
/// own few, and two for each client of the call (its connection and the proxy's
/// connection to the bus). 64 holds more than 16 clients at once.
pub(crate) const MAX_FILES: u64 = 64;

/// The descriptor of the sync pipe in the proxy, which `--fd=3` names.
pub(crate) const READY_FD: RawFd = 3;

/// The arguments of `efr-sbx dbus-proxy`.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct DbusProxyArgs {
    /// The call's bus dir, `$CALL/bus`: the program's working dir and the only dir where
    /// it may make or remove a file.
    #[arg(long)]
    pub(crate) dir: PathBuf,
    /// A bus socket that the program may connect to; one per filtered bus.
    #[arg(long)]
    pub(crate) upstream: Vec<PathBuf>,
    /// The program, an absolute path, and its arguments.
    #[arg(last = true, required = true)]
    pub(crate) command: Vec<OsString>,
}

/// Confines this process and runs the program; returns only when a step failed.
pub(crate) fn main(args: &DbusProxyArgs) -> ExitCode {
    match confine_and_run(args) {
        Ok(never) => match never {},
        Err(error) => {
            let _ = writeln!(io::stderr(), "efr-sbx dbus-proxy: {}", error.chain());
            ExitCode::from(u8::try_from(SETUP_FAILURE_STATUS).unwrap_or(1))
        }
    }
}

/// The steps of the module docs, in order.
fn confine_and_run(args: &DbusProxyArgs) -> Result<Infallible, SbxError> {
    let invalid = |what: &'static str| SbxError::os(what, io::ErrorKind::InvalidInput.into());
    let Some((program, rest)) = args.command.split_first() else {
        return Err(invalid("find the program to run"));
    };
    let program = PathBuf::from(program);
    if !program.is_absolute() {
        return Err(invalid("run a program that is not an absolute path"));
    }
    if !args.dir.is_absolute() || !args.upstream.iter().all(|path| path.is_absolute()) {
        return Err(invalid("use a relative dir or bus path"));
    }
    // NOTE: efrd's spawn marks its other descriptors close-on-exec; the wrapper closes
    // every one all the same, so no descriptor that reached it reaches the proxy.
    fds::close_from(READY_FD)
        .map_err(|error| SbxError::os("close the inherited descriptors", error))?;
    // The sync pipe at fd 3, open across the exec; this process owns no other number.
    let _sync = fds::dup_to(io::stdout().as_fd(), READY_FD, false)
        .map_err(|error| SbxError::os("move the sync pipe to fd 3", error))?;
    // NOTE: the proxy prints its log on stdout; efrd reads one pipe, stderr.
    rustix::stdio::dup2_stdout(io::stderr().as_fd())
        .map_err(|error| SbxError::os("put stderr on stdout", error.into()))?;
    fds::close_from(READY_FD + 1)
        .map_err(|error| SbxError::os("close the descriptors above 3", error))?;
    std::env::set_current_dir(&args.dir)
        .map_err(|error| SbxError::io("change to", &args.dir, error))?;
    let limit = Rlimit { current: Some(MAX_FILES), maximum: Some(MAX_FILES) };
    rustix::process::setrlimit(Resource::Nofile, limit)
        .map_err(|error| SbxError::os("limit the open descriptors", error.into()))?;
    rustix::thread::set_no_new_privs(true)
        .map_err(|error| SbxError::os("set no_new_privs", error.into()))?;
    landlock::restrict(&LandlockPolicy::bus_proxy(&program, &args.dir, &args.upstream), &[])?;
    seccomp::install(&SeccompProfile::bus_proxy())?;
    let error = os::command(&program).args(rest).exec();
    Err(SbxError::Spawn { program, source: error })
}
