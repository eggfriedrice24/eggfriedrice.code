//! `efr-sbx self-test`: the probe's checks from inside a real sandbox (the spec's
//! section 12.1), and the helper the escape suite runs for the system calls a shell
//! cannot make.
//!
//! Each check tries something the sandbox must refuse, or must allow, and prints one
//! JSON line `{ "name", "ok", "detail" }`. A check whose argument is missing is skipped.
//! The exit status is 0 when every check that ran passed. With `--net-view` it runs the
//! checks of a `Grant::NetView` call instead, each of which passes only on its exact
//! errno, so a refusal by another layer (no such name in a private namespace) does not
//! count. With `--proxy` it runs the checks of a call with a proxy plan
//! ([`PROXY_CHECKS`]): a tunnel through the bridge works, and the proxy socket, other
//! addresses and the bridge itself are out of reach, each with its exact result.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::net::{AddressFamily, Protocol, SocketType};
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
    /// Report this process's nice value and IO class (`job_priority`), which a job's
    /// processes inherit from its launcher; a shell cannot read the IO class. The
    /// probe does not run it.
    #[arg(long)]
    pub(crate) job_priority: bool,
    /// Run the checks of a `Grant::NetView` call ([`NET_VIEW_CHECKS`]) instead of
    /// [`CHECKS`].
    #[arg(long)]
    pub(crate) net_view: bool,
    /// The name of an abstract datagram socket made outside: a send to it must fail
    /// with EPERM.
    #[arg(long)]
    pub(crate) abstract_dgram_name: Option<String>,
    /// Run the checks of a call with a proxy plan ([`PROXY_CHECKS`]) instead of
    /// [`CHECKS`].
    #[arg(long)]
    pub(crate) proxy: bool,
    /// The proxy socket inside, `$XRT/efr-proxy.sock`: a connect must fail with
    /// EACCES.
    #[arg(long)]
    pub(crate) proxy_socket: Option<PathBuf>,
    /// An address that a UDP datagram must fail to reach with ENETUNREACH, such as a
    /// name server; more than one may come.
    #[arg(long)]
    pub(crate) udp: Vec<SocketAddr>,
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

/// The checks of a `Grant::NetView` call, in the order they run. Each passes only on
/// its exact errno. `tcp4` runs first, also when `--check` leaves it out: when it does
/// not get `EAFNOSUPPORT`, the profile is not in place, and the self-test stops before
/// it calls `socket()` for any other family or protocol.
pub(crate) const NET_VIEW_CHECKS: &[&str] = &[
    "tcp4",
    "tcp6",
    "udp4",
    "udp6",
    "packet",
    "families",
    "netlink_route",
    "netlink_sock_diag",
    "netlink_other",
    "listen",
    "pidfd_getfd",
    "abstract_stream",
    "abstract_dgram",
    "unix_socket",
];

/// The checks of a call with a proxy plan, in the order they run. Each passes only on
/// its exact result. The probe runs the first four ([`PROBE_PROXY_CHECKS`]).
pub(crate) const PROXY_CHECKS: &[&str] = &[
    "proxy_tcp",
    "proxy_socket_direct",
    "proxy_tcp_other",
    "proxy_bridge_signal",
    "proxy_bridge_environ",
    "proxy_port_taken",
    "proxy_udp",
];

/// The checks of the probe's proxy launch: the tunnel, the socket, another address and
/// the signal scope.
pub(crate) const PROBE_PROXY_CHECKS: &[&str] =
    &["proxy_tcp", "proxy_socket_direct", "proxy_tcp_other", "proxy_bridge_signal"];

/// The host that the tunnel of `proxy_tcp` asks for; the probe's socket answers any.
pub(crate) const PROXY_PROBE_HOST: &str = "probe.test:443";

/// How long `proxy_tcp` waits for each answer of the bridge.
const PROXY_WAIT: Duration = Duration::from_secs(5);

/// The families that a network view call must not open, besides TCP, UDP and packet
/// sockets: Bluetooth, crypto, vsock, XDP and MCTP.
const OTHER_FAMILIES: &[u16] = &[31, 38, 40, 44, 45];

/// The netlink protocols that a network view call must not open: generic, the
/// kernel's uevents and audit.
const OTHER_NETLINK: &[u32] = &[16, 15, 9];

/// Runs the checks and prints them; the exit status says whether all passed.
pub(crate) fn main(args: &SelfTestArgs) -> std::process::ExitCode {
    let mut all = true;
    let mut out = io::stdout().lock();
    if args.proxy {
        for name in PROXY_CHECKS {
            if !args.checks.is_empty() && !args.checks.iter().any(|check| check == name) {
                continue;
            }
            let Some((ok, detail)) = proxy_check(name, args) else { continue };
            all &= ok;
            let line = CheckLine { name: (*name).to_owned(), ok, detail };
            let text = serde_json::to_string(&line).unwrap_or_default();
            let _ = writeln!(out, "{text}");
        }
        return if all { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
    }
    if args.net_view {
        for line in net_view(&args.checks, &mut |name| net_view_check(name, args)) {
            all &= line.ok;
            let text = serde_json::to_string(&line).unwrap_or_default();
            let _ = writeln!(out, "{text}");
        }
        return if all { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
    }
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
    if args.job_priority {
        let (ok, detail) = job_priority();
        all &= ok;
        let line = CheckLine { name: "job_priority".to_owned(), ok, detail };
        let text = serde_json::to_string(&line).unwrap_or_default();
        let _ = writeln!(out, "{text}");
    }
    if all { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

/// A report, not a refusal: this process's nice value and IO class as
/// `nice <n> io_class <c>`, which a job's test compares with what the launcher set.
fn job_priority() -> (bool, String) {
    match (rustix::process::getpriority_process(None), fds::ioprio_class()) {
        (Ok(nice), Ok(class)) => outcome(true, format!("nice {nice} io_class {class}")),
        (nice, class) => outcome(false, format!("{nice:?} {class:?}")),
    }
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

/// The lines of the network view's checks that `selected` names (all when it is
/// empty), with `tcp4` always first: when `tcp4` fails, no other check runs. `check`
/// runs one check; `None` skips it (its argument is missing).
fn net_view(
    selected: &[String],
    check: &mut dyn FnMut(&str) -> Option<(bool, String)>,
) -> Vec<CheckLine> {
    let mut lines = Vec::new();
    for name in NET_VIEW_CHECKS {
        let gate = *name == "tcp4";
        if !gate && !selected.is_empty() && !selected.iter().any(|check| check == name) {
            continue;
        }
        let Some((ok, detail)) = check(name) else { continue };
        lines.push(CheckLine { name: (*name).to_owned(), ok, detail });
        if gate && !ok {
            break;
        }
    }
    lines
}

/// A socket of `family`, `kind` and `protocol`, as `socket()` gives it.
fn open_socket(family: u16, kind: SocketType, protocol: u32) -> Result<OwnedFd, Errno> {
    let protocol = std::num::NonZeroU32::new(protocol).map(Protocol::from_raw);
    rustix::net::socket(AddressFamily::from_raw(family), kind, protocol)
}

/// `(ok, detail)` for a `socket()` call that must fail with exactly `expected`.
fn refused_with(done: Result<OwnedFd, Errno>, expected: Errno) -> (bool, String) {
    match done {
        Err(errno) => (errno == expected, format!("errno {}", errno.raw_os_error())),
        Ok(_) => (false, "opened".to_owned()),
    }
}

/// `Some((ok, detail))` for one check of [`NET_VIEW_CHECKS`], or `None` when its
/// argument is missing.
fn net_view_check(name: &str, args: &SelfTestArgs) -> Option<(bool, String)> {
    let inet = AddressFamily::INET.as_raw();
    let inet6 = AddressFamily::INET6.as_raw();
    let netlink = AddressFamily::NETLINK.as_raw();
    let unsupported = |family, kind| refused_with(open_socket(family, kind, 0), Errno::AFNOSUPPORT);
    let errno = |error: &io::Error| error.raw_os_error().unwrap_or_default();
    Some(match name {
        "tcp4" => unsupported(inet, SocketType::STREAM),
        "tcp6" => unsupported(inet6, SocketType::STREAM),
        "udp4" => unsupported(inet, SocketType::DGRAM),
        "udp6" => unsupported(inet6, SocketType::DGRAM),
        "packet" => unsupported(AddressFamily::PACKET.as_raw(), SocketType::RAW),
        "families" => {
            let results: Vec<(u16, (bool, String))> = OTHER_FAMILIES
                .iter()
                .map(|family| (*family, unsupported(*family, SocketType::STREAM)))
                .collect();
            let ok = results.iter().all(|(_, (ok, _))| *ok);
            let detail = results
                .iter()
                .map(|(family, (_, detail))| format!("{family}: {detail}"))
                .collect::<Vec<_>>()
                .join(", ");
            (ok, detail)
        }
        "netlink_route" | "netlink_sock_diag" => {
            let protocol = if name == "netlink_route" { 0 } else { 4 };
            match open_socket(netlink, SocketType::RAW, protocol) {
                Ok(_) => (true, "opens".to_owned()),
                Err(errno) => (false, format!("errno {}", errno.raw_os_error())),
            }
        }
        "netlink_other" => {
            let results: Vec<(u32, (bool, String))> = OTHER_NETLINK
                .iter()
                .map(|protocol| {
                    let done = open_socket(netlink, SocketType::RAW, *protocol);
                    (*protocol, refused_with(done, Errno::PROTONOSUPPORT))
                })
                .collect();
            let ok = results.iter().all(|(_, (ok, _))| *ok);
            let detail = results
                .iter()
                .map(|(protocol, (_, detail))| format!("{protocol}: {detail}"))
                .collect::<Vec<_>>()
                .join(", ");
            (ok, detail)
        }
        "listen" => {
            // Autobind: the kernel picks an abstract name, so the check takes no real one.
            let unix = AddressFamily::UNIX.as_raw();
            let bound = open_socket(unix, SocketType::STREAM, 0).and_then(|socket| {
                rustix::net::bind(&socket, &rustix::net::SocketAddrUnix::new_unnamed())?;
                Ok(socket)
            });
            match bound {
                Ok(socket) => {
                    let done = rustix::net::listen(&socket, 1);
                    let ok = done == Err(Errno::PERM);
                    (ok, format!("{:?}", done.map_err(|errno| errno.raw_os_error())))
                }
                Err(errno) => (false, format!("no bound socket: errno {}", errno.raw_os_error())),
            }
        }
        "pidfd_getfd" => {
            let own = rustix::process::getpid();
            match rustix::process::pidfd_open(own, rustix::process::PidfdFlags::empty()) {
                Ok(pidfd) => {
                    let flags = rustix::process::PidfdGetfdFlags::empty();
                    let done = rustix::process::pidfd_getfd(&pidfd, 0, flags);
                    refused_with(done, Errno::PERM)
                }
                Err(errno) => (false, format!("no pidfd: errno {}", errno.raw_os_error())),
            }
        }
        "abstract_stream" => {
            let name = args.abstract_name.as_ref()?;
            let done = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
                .and_then(|address| UnixStream::connect_addr(&address));
            let ok = done.as_ref().is_err_and(|error| errno(error) == Errno::PERM.raw_os_error());
            (ok, format!("{:?}", done.map(drop)))
        }
        "abstract_dgram" => {
            let name = args.abstract_dgram_name.as_ref()?;
            let done = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
                .and_then(|address| {
                    UnixDatagram::unbound()?.send_to_addr(b"efr-sbx self-test", &address)
                });
            let ok = done.as_ref().is_err_and(|error| errno(error) == Errno::PERM.raw_os_error());
            (ok, format!("{done:?}"))
        }
        "unix_socket" => return run(name, args),
        _ => return None,
    })
}

/// `Some((ok, detail))` for one check of [`PROXY_CHECKS`], or `None` when its argument
/// is missing.
fn proxy_check(name: &str, args: &SelfTestArgs) -> Option<(bool, String)> {
    let errno = |error: &io::Error| error.raw_os_error().unwrap_or_default();
    let is = |done: &io::Result<()>, expected: Errno| {
        done.as_ref().is_err_and(|error| errno(error) == expected.raw_os_error())
    };
    Some(match name {
        "proxy_tcp" => match tunnel() {
            Ok(detail) => (true, detail),
            Err(detail) => (false, detail),
        },
        "proxy_socket_direct" => match socket_path(args.proxy_socket.as_ref()?) {
            Err(error) => (false, format!("no path to the socket: {error:?}")),
            Ok((dir, path)) => {
                let done = UnixStream::connect(&path).map(drop);
                drop(dir);
                (is(&done, Errno::ACCESS), format!("{done:?}"))
            }
        },
        "proxy_tcp_other" => {
            let address = args.tcp.unwrap_or_else(|| {
                efr_sandbox::SELF_TEST_TCP_ADDR.parse().unwrap_or(SocketAddr::from(([0; 4], 9)))
            });
            let done = TcpStream::connect_timeout(&address, Duration::from_secs(2)).map(drop);
            (is(&done, Errno::NETUNREACH), format!("{address}: {done:?}"))
        }
        "proxy_bridge_signal" => match bridge_pid().and_then(|pid| Some((pid, inner_pid(pid)?))) {
            None => (false, "no bridge of this call".to_owned()),
            Some((host, pid)) => {
                // NOTE: kill() takes the pid in the call's own pid namespace.
                let done = rustix::process::Pid::from_raw(pid)
                    .map(rustix::process::test_kill_process)
                    .unwrap_or(Err(Errno::SRCH));
                (done == Err(Errno::PERM), format!("{pid} (host {host}): {done:?}"))
            }
        },
        "proxy_bridge_environ" => match bridge_pid() {
            None => (false, "no bridge of this call".to_owned()),
            Some(pid) => bridge_environ(pid),
        },
        "proxy_port_taken" => {
            let done = TcpListener::bind((Ipv4Addr::LOCALHOST, efr_sandbox::PROXY_PORT)).map(drop);
            (is(&done, Errno::ADDRINUSE), format!("{done:?}"))
        }
        "proxy_udp" => {
            if args.udp.is_empty() {
                return None;
            }
            let results: Vec<(SocketAddr, io::Result<()>)> = args
                .udp
                .iter()
                .map(|address| {
                    let local: SocketAddr = if address.is_ipv4() {
                        (Ipv4Addr::UNSPECIFIED, 0).into()
                    } else {
                        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
                    };
                    let done = UdpSocket::bind(local)
                        .and_then(|socket| socket.send_to(b"efr-sbx self-test", address))
                        .map(drop);
                    (*address, done)
                })
                .collect();
            let ok = results.iter().all(|(_, done)| is(done, Errno::NETUNREACH));
            let detail = results
                .iter()
                .map(|(address, done)| format!("{address}: {done:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            (ok, detail)
        }
        _ => return None,
    })
}

/// A tunnel through the bridge: `CONNECT` [`PROXY_PROBE_HOST`], the answer `200`, then
/// one byte there and back.
fn tunnel() -> Result<String, String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, efr_sandbox::PROXY_PORT));
    let mut stream = TcpStream::connect_timeout(&address, PROXY_WAIT)
        .map_err(|error| format!("no bridge on {address}: {error:?}"))?;
    stream
        .set_read_timeout(Some(PROXY_WAIT))
        .map_err(|error| format!("no read timeout: {error:?}"))?;
    let head = format!("CONNECT {PROXY_PROBE_HOST} HTTP/1.1\r\nHost: {PROXY_PROBE_HOST}\r\n\r\n");
    stream.write_all(head.as_bytes()).map_err(|error| format!("no write: {error:?}"))?;
    let mut answer = Vec::new();
    let mut byte = [0_u8; 1];
    while !answer.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) if answer.len() < 4096 => answer.push(byte[0]),
            other => return Err(format!("no answer: {other:?} after {answer:?}")),
        }
    }
    let text = String::from_utf8_lossy(&answer).into_owned();
    if !text.starts_with("HTTP/1.1 200") {
        return Err(format!("the answer was {text:?}"));
    }
    stream.write_all(b"e").map_err(|error| format!("no write in the tunnel: {error:?}"))?;
    match stream.read(&mut byte) {
        Ok(1) if byte == *b"e" => Ok(text.lines().next().unwrap_or_default().to_owned()),
        other => Err(format!("no echo in the tunnel: {other:?}")),
    }
}

/// The bridge of this call: the process named `efr-bridge` whose parent is an ancestor
/// of this process, the namespace's init. The host's `/proc` shows every process, also
/// the bridges of other calls; their parents are other inits.
pub(crate) fn bridge_pid() -> Option<i32> {
    // NOTE: /proc is the host's procfs (the plan never mounts a new one), so its pids are
    // the host's; getpid() would give this process's pid in the call's namespace.
    let mut ancestors = Vec::new();
    let own = fs::read_link("/proc/self").ok()?;
    let mut pid: i32 = own.to_str()?.parse().ok()?;
    while let Some(parent) = crate::job::parent_pid(pid).filter(|parent| *parent > 0) {
        ancestors.push(parent);
        pid = parent;
    }
    let entries = fs::read_dir("/proc").ok()?;
    entries.flatten().find_map(|entry| {
        let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
        let name = fs::read_to_string(entry.path().join("comm")).ok()?;
        let ours = name.trim() == crate::bridge::BRIDGE_NAME
            && crate::job::parent_pid(pid).is_some_and(|parent| ancestors.contains(&parent));
        ours.then_some(pid)
    })
}

/// The pid in the innermost pid namespace of the process that the host's `/proc` names
/// `pid`: the last number of the `NSpid` line of its status.
fn inner_pid(pid: i32) -> Option<i32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find_map(|line| line.strip_prefix("NSpid:"))?;
    line.split_whitespace().last()?.parse().ok()
}

/// The bridge's environment from inside: the kernel gives the files of a process that is
/// not dumpable to root, which reads as the overflow uid in the call's user namespace,
/// and the read fails with EACCES.
fn bridge_environ(pid: i32) -> (bool, String) {
    use std::os::unix::fs::MetadataExt;
    let path = format!("/proc/{pid}/environ");
    let overflow = fs::read_to_string("/proc/sys/kernel/overflowuid")
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    let owner = fs::metadata(&path).map(|meta| meta.uid());
    let read = fs::read(&path).map(|bytes| bytes.len());
    let refused = read
        .as_ref()
        .is_err_and(|error| error.raw_os_error() == Some(Errno::ACCESS.raw_os_error()));
    let owned = matches!((&owner, overflow), (Ok(uid), Some(overflow)) if *uid == overflow);
    (owned && refused, format!("owner {owner:?}, overflow uid {overflow:?}, read {read:?}"))
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
