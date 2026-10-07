//! `efr-sbx self-test`: the probe's checks from inside a real sandbox (the spec's
//! section 12.1), and the helper the escape suite runs for the system calls a shell
//! cannot make.
//!
//! Each check tries something the sandbox must refuse, or must allow, and prints one
//! JSON line `{ "name", "ok", "detail" }`. A check whose argument is missing is skipped.
//! The exit status is 0 when every check that ran passed.

use std::fs;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::fds;

/// The arguments: what to try and where.
#[derive(Debug, Clone, Default, clap::Args)]
pub(crate) struct SelfTestArgs {
    /// Run only these checks; all when none is named.
    #[arg(long = "check")]
    pub(crate) checks: Vec<String>,
    /// A write root: a write there must work.
    #[arg(long)]
    pub(crate) project: Option<PathBuf>,
    /// A path in a read-only place: a write there must fail with EROFS.
    #[arg(long)]
    pub(crate) outside: Option<PathBuf>,
    /// A path on a writable mount without a Landlock write rule: EACCES.
    #[arg(long)]
    pub(crate) landlock_only: Option<PathBuf>,
    /// A masked directory: it must read as empty.
    #[arg(long)]
    pub(crate) masked: Option<PathBuf>,
    /// A Unix socket made outside: connecting must fail with EACCES.
    #[arg(long)]
    pub(crate) socket: Option<PathBuf>,
    /// The name of an abstract socket made outside: connecting must fail.
    #[arg(long)]
    pub(crate) abstract_name: Option<String>,
    /// A process outside: signals and its environ must be refused.
    #[arg(long)]
    pub(crate) outside_pid: Option<i32>,
    /// A pseudo-terminal opened outside: it must not be visible.
    #[arg(long)]
    pub(crate) other_pts: Option<PathBuf>,
    /// A directory of a cache overlay: a write there must work.
    #[arg(long)]
    pub(crate) cache: Option<PathBuf>,
    /// The cache is read-only (`sandbox.cache_mode = "readonly"`): a write there must
    /// fail with EROFS instead.
    #[arg(long)]
    pub(crate) cache_read_only: bool,
    /// The address that TCP must fail to reach at once.
    #[arg(long)]
    pub(crate) tcp: Option<SocketAddr>,
}

/// One check's outcome, as the self-test prints it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CheckLine {
    /// The check.
    pub(crate) name: String,
    /// True when the sandbox behaved as it must.
    pub(crate) ok: bool,
    /// What happened.
    pub(crate) detail: String,
}

/// The names of every check, in the order they run.
pub(crate) const CHECKS: &[&str] = &[
    "write_inside",
    "write_outside",
    "write_landlock_only",
    "mask_empty",
    "unix_socket",
    "abstract_socket",
    "signal_outside",
    "proc_environ",
    "tcp",
    "io_uring",
    "vsock",
    "nested_userns",
    "other_pts",
    "tiocsti",
    "tty",
    "cache_write",
];

/// Runs the checks and prints them; the exit status says whether all passed.
pub(crate) fn main(args: &SelfTestArgs) -> std::process::ExitCode {
    let mut all = true;
    let mut out = io::stdout().lock();
    for name in CHECKS {
        if !args.checks.is_empty() && !args.checks.iter().any(|check| check == name) {
            continue;
        }
        let Some((ok, detail)) = run(name, args) else { continue };
        all &= ok;
        let line = CheckLine { name: (*name).to_owned(), ok, detail };
        let text = serde_json::to_string(&line).unwrap_or_default();
        let _ = writeln!(out, "{text}");
    }
    if all { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

/// `Some((ok, detail))`, or `None` when the check's argument is missing.
fn run(name: &str, args: &SelfTestArgs) -> Option<(bool, String)> {
    let errno = |error: &io::Error| error.raw_os_error().unwrap_or_default();
    Some(match name {
        "write_inside" => {
            let path = args.project.as_ref()?.join(".efr-self-test");
            let done = fs::write(&path, b"x").and_then(|()| fs::remove_file(&path));
            outcome(done.is_ok(), format!("{done:?}"))
        }
        "write_outside" => {
            let done = fs::write(args.outside.as_ref()?, b"x");
            let refused =
                done.as_ref().is_err_and(|error| errno(error) == Errno::ROFS.raw_os_error());
            outcome(refused, format!("{done:?}"))
        }
        "write_landlock_only" => {
            let done = fs::write(args.landlock_only.as_ref()?, b"x");
            let refused =
                done.as_ref().is_err_and(|error| errno(error) == Errno::ACCESS.raw_os_error());
            outcome(refused, format!("{done:?}"))
        }
        "mask_empty" => {
            let entries = fs::read_dir(args.masked.as_ref()?).map(Iterator::count);
            outcome(matches!(entries, Ok(0)), format!("{entries:?}"))
        }
        "unix_socket" => match socket_path(args.socket.as_ref()?) {
            // NOTE: a refusal here must not count as the refused connect.
            Err(error) => outcome(false, format!("no path to the socket: {error:?}")),
            Ok((dir, path)) => {
                let done = UnixStream::connect(&path);
                drop(dir);
                let refused =
                    done.as_ref().is_err_and(|error| errno(error) == Errno::ACCESS.raw_os_error());
                outcome(refused, format!("{:?}", done.map(drop)))
            }
        },
        "abstract_socket" => {
            let name = args.abstract_name.as_ref()?;
            let done = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
                .and_then(|address| UnixStream::connect_addr(&address));
            outcome(done.is_err(), format!("{:?}", done.map(drop)))
        }
        "signal_outside" => {
            let pid = rustix::process::Pid::from_raw(args.outside_pid?)?;
            let done = rustix::process::test_kill_process(pid);
            outcome(done.is_err(), format!("{done:?}"))
        }
        "proc_environ" => {
            let done = fs::read(format!("/proc/{}/environ", args.outside_pid?));
            outcome(done.is_err(), format!("{:?}", done.map(|bytes| bytes.len())))
        }
        "tcp" => {
            let done = TcpStream::connect_timeout(args.tcp.as_ref()?, Duration::from_secs(2));
            outcome(done.is_err(), format!("{:?}", done.map(drop)))
        }
        "io_uring" => match fds::io_uring_blocked() {
            Ok(code) => outcome(code == Errno::PERM.raw_os_error(), format!("errno {code}")),
            Err(what) => outcome(false, what),
        },
        "vsock" => {
            use rustix::net::{AddressFamily, SocketType};
            let done = rustix::net::socket(AddressFamily::VSOCK, SocketType::STREAM, None);
            let refused = matches!(done, Err(Errno::AFNOSUPPORT));
            outcome(refused, format!("{:?}", done.map(drop)))
        }
        "nested_userns" => {
            let done = fds::userns_blocked();
            outcome(done.is_ok(), format!("{done:?}"))
        }
        "other_pts" => {
            let path = args.other_pts.as_ref()?;
            let seen = Path::new(path).exists();
            outcome(!seen, format!("{} visible: {seen}", path.display()))
        }
        "tiocsti" => {
            let flags = rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY;
            match rustix::pty::openpt(flags) {
                Ok(master) => match fds::tiocsti_blocked(master.as_fd()) {
                    Ok(code) => {
                        outcome(code == Errno::PERM.raw_os_error(), format!("errno {code}"))
                    }
                    Err(what) => outcome(false, what),
                },
                Err(error) => outcome(false, format!("no pty: {error:?}")),
            }
        }
        "tty" => match fs::OpenOptions::new().read(true).write(true).open("/dev/tty") {
            Ok(_) => outcome(true, "opens".to_owned()),
            // Without a controlling terminal (efrd runs the probe as a service) there
            // is nothing to open; the check then proves nothing and passes.
            Err(error) if errno(&error) == Errno::NXIO.raw_os_error() => {
                outcome(true, "no controlling terminal".to_owned())
            }
            Err(error) => outcome(false, format!("{error:?}")),
        },
        "cache_write" => {
            let path = args.cache.as_ref()?.join(".efr-self-test");
            let done = fs::write(&path, b"x");
            let ok = if args.cache_read_only {
                done.as_ref().is_err_and(|error| errno(error) == Errno::ROFS.raw_os_error())
            } else {
                done.is_ok()
            };
            outcome(ok, format!("{done:?}"))
        }
        _ => return None,
    })
}

/// The most bytes of a socket's path: `sun_path` holds 108 with the closing NUL.
const MAX_SOCKET_PATH: usize = 107;

/// A path to the socket `path` that fits in a socket address: `path` itself, or, when
/// it is too long, `/proc/self/fd/<n>/<name>` through a descriptor of its directory,
/// which the caller keeps open while it binds or connects. A deep state dir then still
/// gets the socket check.
pub(crate) fn socket_path(path: &Path) -> io::Result<(Option<OwnedFd>, PathBuf)> {
    if path.as_os_str().len() <= MAX_SOCKET_PATH {
        return Ok((None, path.to_path_buf()));
    }
    let invalid = || io::Error::from(io::ErrorKind::InvalidInput);
    let (dir, name) = (path.parent().ok_or_else(invalid)?, path.file_name().ok_or_else(invalid)?);
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let fd = rustix::fs::open(dir, flags, Mode::empty())?;
    let short = PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd())).join(name);
    Ok((Some(fd), short))
}

fn outcome(ok: bool, detail: String) -> (bool, String) {
    (ok, detail)
}

#[cfg(test)]
mod tests;
