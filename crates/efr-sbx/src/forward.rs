//! `efr-sbx forward`: the forwarder of a contained job with port forwards.
//!
//! The job's network namespace has loopback only, so a server that the job starts is
//! out of reach of the host. The forwarder reads the job's own `/proc/self/net/tcp` and
//! `tcp6` every `poll_ms`, and reports each stable loopback listen to efrd over the
//! job's forward socket (`$XRT/efr-forward.sock`). efrd binds the same port on the
//! host's `127.0.0.1` and, for each host connection, asks the forwarder to connect to
//! the job's server. The child of the job cannot reach the socket itself: its Landlock
//! domain has no `RESOLVE_UNIX` rule for it.
//!
//! The inner stage starts it as the bridge, with the starter and the setup of
//! `helper.rs`, but binds nothing for it:
//!
//! 1. The starter, `efr-sbx forward --socket P ...`, runs the server, `efr-sbx forward
//!    --serve --ready-fd 1 ...`, and waits for its ready byte.
//! 2. The server closes every descriptor from 3 up, makes a session of its own, names
//!    itself `efr-forward`, opens `/proc/self/net/tcp` and `tcp6` (a missing `tcp6`, for
//!    a kernel with IPv6 off, is an empty table) and reads the namespace's ephemeral
//!    range, makes itself not dumpable, and applies its own Landlock domain
//!    (`LandlockPolicy::forward`: no file right, `RESOLVE_UNIX` on the socket only, the
//!    TCP bind right handled with no rule) and seccomp profile
//!    (`SeccompProfile::forward`: only Unix, IPv4 and IPv6 sockets; no `bind`, `listen`
//!    or `execve`). Only then does it write `R`.
//! 3. It connects to the socket and sends `Hello`: the control connection. A poll
//!    thread reads both tables again from offset 0 at each poll, through the
//!    descriptors that it opened before its domain, and sends `Listen` and `Gone` from
//!    `efr_sandbox::Debounce`. The main thread reads efrd's `Open` frames.
//! 4. For each `Open`, a thread connects to the frame's address and port inside the
//!    namespace, opens a new connection to the socket and sends `Data` with status 0,
//!    then copies bytes both ways; or `Data` with the errno of the connect. It refuses
//!    a port below 1024 and the bridge's 3128 in a proxy job (`EPERM`), and more than
//!    `MAX_FORWARD_CONNECTIONS` open connections (`EBUSY`).
//!
//! Unlike the bridge, a forwarder that does not start never fails the job: the inner
//! stage writes nothing about it, and efrd sees that no `Hello` came. The forwarder ends
//! when efrd closes the control connection, when efrd sends a frame that is not a valid
//! `Open`, and with the job's namespace.
//!
//! `--check` runs the server's setup and checks its domain from inside a job: not
//! dumpable, a write, `bind`, `listen` and `execve` refused, a read of the tables and a
//! TCP connect still work.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::ops::RangeInclusive;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use efr_sandbox::{
    Change, Debounce, FRAME_LEN, ForwardPlan, Frame, FrameKind, InnerForward, LandlockPolicy,
    MAX_FORWARD_CONNECTIONS, MAX_JOB_FORWARDS, PortRefusal, SeccompProfile, TcpRow,
    loopback_listens, parse_tcp_table, refusal,
};
use rustix::io::Errno;

use crate::error::SbxError;
use crate::helper::{self, Confinement, Helper, Slot, Step, Take};
use crate::os;
use crate::self_test::socket_path;

/// The name of the server's process, which a job's stop spares (`job.rs`).
pub(crate) const FORWARD_NAME: &str = "efr-forward";

/// The job's IPv4 table.
const TCP: &str = "/proc/self/net/tcp";

/// The job's IPv6 table; missing on a kernel with IPv6 off.
const TCP6: &str = "/proc/self/net/tcp6";

/// The namespace's ephemeral range.
const PORT_RANGE: &str = "/proc/sys/net/ipv4/ip_local_port_range";

/// The kernel's ephemeral range of a new network namespace, when the file does not
/// read. A new namespace never copies the host's range.
const DEFAULT_EPHEMERAL: RangeInclusive<u16> = 32768..=60999;

/// The most bytes of one table that a poll reads: about 27 000 rows.
const MAX_TABLE_BYTES: usize = 4 * 1024 * 1024;

/// How long the connect of an `Open` may take. efrd waits 5 s for the `Data`.
const CONNECT_WAIT: Duration = Duration::from_secs(2);

/// The status of a `Data` for a port that the forwarder never opens: `EPERM`.
const STATUS_REFUSED: u8 = 1;

/// The status of a `Data` when every place is taken: `EBUSY`.
const STATUS_BUSY: u8 = 16;

/// The status of a `Data` for a connect that failed with no errno: `EIO`.
const STATUS_FAILED: u8 = 5;

/// The arguments of `efr-sbx forward`.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct ForwardArgs {
    /// The job's forward socket inside, `$XRT/efr-forward.sock`.
    #[arg(long)]
    pub(crate) socket: PathBuf,
    /// The time between two polls of the job's tcp tables.
    #[arg(long, default_value_t = 500)]
    pub(crate) poll_ms: u32,
    /// How long a port must listen before it counts.
    #[arg(long, default_value_t = 1000)]
    pub(crate) stable_ms: u32,
    /// How long a port of the ephemeral range must listen before it counts.
    #[arg(long, default_value_t = 10_000)]
    pub(crate) ephemeral_stable_ms: u32,
    /// The most ports that efr forwards for the job.
    #[arg(long, default_value_t = MAX_JOB_FORWARDS)]
    pub(crate) max_ports: u8,
    /// The job has a proxy plan: port 3128 is the bridge's.
    #[arg(long)]
    pub(crate) proxy: bool,
    /// Run the server: the setup, the ready byte, then the polls and the connections.
    #[arg(long)]
    pub(crate) serve: bool,
    /// The server's ready pipe: 1.
    #[arg(long)]
    pub(crate) ready_fd: Option<i32>,
    /// Run the server's setup and check its domain; exit 0 when every check passes.
    #[arg(long)]
    pub(crate) check: bool,
    /// With `--check`: a directory that the job may write; a new file there must fail
    /// with EACCES.
    #[arg(long, requires = "check")]
    pub(crate) write: Option<PathBuf>,
    /// End the server's setup before this step, for the tests.
    #[arg(long, value_enum)]
    pub(crate) fail_setup: Option<Step>,
}

impl ForwardArgs {
    /// The forward plan of these arguments, with the socket inside.
    fn plan(&self) -> ForwardPlan {
        ForwardPlan {
            socket: self.socket.clone(),
            poll_ms: self.poll_ms,
            stable_ms: self.stable_ms,
            ephemeral_stable_ms: self.ephemeral_stable_ms,
            max_ports: self.max_ports,
            proxy: self.proxy,
        }
    }
}

/// The starter's arguments for the inner stage's `forward`: `efr-sbx forward --socket
/// P` and the plan's timings and limit.
pub(crate) fn argv(forward: &InnerForward) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        "forward".into(),
        "--socket".into(),
        forward.socket.clone().into_os_string(),
        "--poll-ms".into(),
        forward.poll_ms.to_string().into(),
        "--stable-ms".into(),
        forward.stable_ms.to_string().into(),
        "--ephemeral-stable-ms".into(),
        forward.ephemeral_stable_ms.to_string().into(),
        "--max-ports".into(),
        forward.max_ports.to_string().into(),
    ];
    if forward.proxy {
        argv.push("--proxy".into());
    }
    argv
}

/// Runs the starter, the server or the check.
pub(crate) fn main(args: &ForwardArgs) -> ExitCode {
    let done = if args.check {
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
            let _ = writeln!(io::stderr(), "efr-sbx forward: {}", error.chain());
            ExitCode::FAILURE
        }
    }
}

/// The starter: runs the server and waits for its ready byte.
fn start(args: &ForwardArgs) -> Result<(), SbxError> {
    let inner = InnerForward::new(&args.plan(), args.socket.clone());
    let mut argv = argv(&inner);
    argv.extend(["--serve".into(), "--ready-fd".into(), "1".into()]);
    if let Some(step) = args.fail_setup {
        argv.extend(["--fail-setup".into(), step.word().into()]);
    }
    helper::start(Helper::Forwarder, argv, None)
}

/// The forwarder's confinement.
fn confinement(args: &ForwardArgs) -> Confinement {
    Confinement {
        helper: Helper::Forwarder,
        landlock: LandlockPolicy::forward(&args.socket),
        seccomp: SeccompProfile::forward(),
        fail_setup: args.fail_setup,
    }
}

/// What the server opens before its own domain.
#[derive(Debug)]
struct Tables {
    /// `/proc/self/net/tcp`.
    tcp: File,
    /// `/proc/self/net/tcp6`, when the kernel has IPv6.
    tcp6: Option<File>,
    /// The namespace's ephemeral range.
    ephemeral: RangeInclusive<u16>,
    /// The path to connect to the socket.
    socket: PathBuf,
    /// The socket's dir when its path is too long for a socket address.
    _dir: Option<OwnedFd>,
}

/// Opens the tables, reads the ephemeral range and finds the socket.
fn open_tables(socket: &Path) -> Result<Tables, SbxError> {
    let tcp = File::open(TCP).map_err(|error| SbxError::io("open", TCP, error))?;
    let tcp6 = open_optional(Path::new(TCP6))?;
    let ephemeral = std::fs::read_to_string(PORT_RANGE)
        .ok()
        .and_then(|text| parse_range(&text))
        .unwrap_or(DEFAULT_EPHEMERAL);
    let (dir, socket) = socket_path(socket).map_err(|error| SbxError::io("find", socket, error))?;
    Ok(Tables { tcp, tcp6, ephemeral, socket, _dir: dir })
}

/// `path` opened for reading, or `None` when it does not exist.
fn open_optional(path: &Path) -> Result<Option<File>, SbxError> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SbxError::io("open", path, error)),
    }
}

/// The range of `ip_local_port_range`: two numbers, the low one first.
fn parse_range(text: &str) -> Option<RangeInclusive<u16>> {
    let mut words = text.split_whitespace();
    let low: u16 = words.next()?.parse().ok()?;
    let high: u16 = words.next()?.parse().ok()?;
    (words.next().is_none() && low <= high).then_some(low..=high)
}

/// Reads `file` again from offset 0 into `text`, at most [`MAX_TABLE_BYTES`].
fn read_table(file: &File, text: &mut Vec<u8>) -> io::Result<()> {
    text.clear();
    let mut chunk = vec![0_u8; 64 * 1024];
    let mut offset = 0_u64;
    while text.len() < MAX_TABLE_BYTES {
        let read = match file.read_at(&mut chunk, offset) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        text.extend_from_slice(chunk.get(..read).unwrap_or_default());
        offset = offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    Ok(())
}

/// The rows of one table, or `None` when the read failed.
fn rows(file: &File, text: &mut Vec<u8>, v6: bool) -> Option<Vec<TcpRow>> {
    read_table(file, text).ok()?;
    Some(parse_tcp_table(&String::from_utf8_lossy(text), v6))
}

/// The server: the setup, the ready byte, then the control connection.
fn serve_main(args: &ForwardArgs) -> Result<(), SbxError> {
    let take = Take { listen_fd: None, ready_fd: args.ready_fd };
    let (streams, tables) =
        helper::setup(&confinement(args), Some(take), || open_tables(&args.socket))?;
    let Some(ready) = streams.ready else {
        return Err(SbxError::os("take the ready pipe", io::ErrorKind::InvalidInput.into()));
    };
    helper::report_ready(ready)?;
    serve(tables, &args.plan())
}

/// Sends `Hello`, starts the poll thread and answers efrd's frames until efrd closes
/// the control connection.
fn serve(tables: Tables, plan: &ForwardPlan) -> Result<(), SbxError> {
    let Tables { tcp, tcp6, ephemeral, socket, _dir } = tables;
    let mut control =
        UnixStream::connect(&socket).map_err(|error| SbxError::io("connect to", &socket, error))?;
    control
        .write_all(&Frame::hello().encode())
        .map_err(|error| SbxError::os("send the forwarder's hello", error))?;
    let reports =
        control.try_clone().map_err(|error| SbxError::os("copy the control connection", error))?;
    let debounce = Debounce::new(plan, ephemeral);
    let every = Duration::from_millis(u64::from(plan.poll_ms));
    std::thread::Builder::new()
        .name("efr-fwd-poll".to_owned())
        .stack_size(helper::COPY_STACK)
        .spawn(move || poll(reports, &tcp, tcp6.as_ref(), debounce, every))
        .map_err(|error| SbxError::os("start the forwarder's poll thread", error))?;
    let open = Arc::new(AtomicUsize::new(0));
    let mut bytes = [0_u8; FRAME_LEN];
    loop {
        // NOTE: `read_exact` retries an interrupted read itself, so an error here means
        // that efrd closed the control connection: the forwards of this job end.
        if control.read_exact(&mut bytes).is_err() {
            return Ok(());
        }
        let frame = Frame::decode(&bytes)?;
        if frame.kind != FrameKind::Open {
            return Err(SbxError::os("read efrd's frame", io::ErrorKind::InvalidData.into()));
        }
        answer(frame, &socket, &open, plan.proxy);
    }
}

/// Polls the tables every `every` and reports each change on `control`. A failed
/// report ends the control connection, so the main thread ends too.
fn poll(
    mut control: UnixStream,
    tcp: &File,
    tcp6: Option<&File>,
    mut debounce: Debounce,
    every: Duration,
) {
    let start = os::now();
    let mut text = Vec::new();
    loop {
        let v4 = rows(tcp, &mut text, false);
        let v6 = match tcp6 {
            Some(tcp6) => rows(tcp6, &mut text, true),
            None => Some(Vec::new()),
        };
        // NOTE: a failed read skips the poll: an empty table would report every port
        // gone.
        if let (Some(v4), Some(v6)) = (v4, v6) {
            let now_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            for change in debounce.observe(now_ms, &loopback_listens(&v4, &v6)) {
                let frame = match change {
                    Change::Listen(listen) => Frame::listen(listen),
                    Change::Gone(listen) => Frame::gone(listen),
                };
                if control.write_all(&frame.encode()).is_err() {
                    let _ = control.shutdown(Shutdown::Both);
                    return;
                }
            }
        }
        os::sleep(every);
    }
}

/// The status of a `Data` that the forwarder gives before any connect, or `None` when
/// it connects: a port below 1024 or the bridge's port in a proxy job is refused, and
/// with no free place the connection waits for none.
fn status_before_connect(port: u16, proxy: bool, place: bool) -> Option<u8> {
    match refusal(port, proxy) {
        Some(PortRefusal::BelowMin | PortRefusal::Bridge) => Some(STATUS_REFUSED),
        _ if !place => Some(STATUS_BUSY),
        _ => None,
    }
}

/// The status of a `Data` for a connect that failed with `error`.
fn status_of(error: &io::Error) -> u8 {
    error
        .raw_os_error()
        .and_then(|errno| u8::try_from(errno).ok())
        .filter(|errno| *errno != 0)
        .unwrap_or(STATUS_FAILED)
}

/// Answers one `Open` on a thread of its own: the connect inside, a new connection to
/// the socket with its `Data`, then the copies.
fn answer(frame: Frame, socket: &Path, open: &Arc<AtomicUsize>, proxy: bool) {
    let target = frame.target();
    let id = frame.id;
    let slot = Slot::take(open, MAX_FORWARD_CONNECTIONS);
    let socket = socket.to_path_buf();
    let _ = helper::spawn_copy(Helper::Forwarder, move || {
        let (tcp, status) = match status_before_connect(target.port, proxy, slot.is_some()) {
            Some(status) => (None, status),
            None => {
                let address = SocketAddr::new(target.addr, target.port);
                match TcpStream::connect_timeout(&address, CONNECT_WAIT) {
                    Ok(tcp) => (Some(tcp), 0),
                    Err(error) => (None, status_of(&error)),
                }
            }
        };
        let Ok(mut unix) = UnixStream::connect(&socket) else { return };
        if unix.write_all(&Frame::data(id, status).encode()).is_err() {
            return;
        }
        if let Some(tcp) = tcp {
            helper::copy_both(Helper::Forwarder, tcp, unix);
        }
        drop(slot);
    });
}

/// `--check`: the server's setup, then the checks of its domain. Prints one JSON line
/// per check, as the self-test does.
fn check(args: &ForwardArgs) -> Result<(), SbxError> {
    let (streams, tables) = helper::setup(&confinement(args), None, || open_tables(&args.socket))?;
    let mut lines = vec![helper::dumpable_line()];
    if let Some(dir) = &args.write {
        lines.push(helper::write_line(dir, ".efr-forward-write"));
    }
    let mut text = Vec::new();
    let read = rows(&tables.tcp, &mut text, false);
    let header = String::from_utf8_lossy(&text).contains("local_address");
    lines.push(helper::line(
        "tables",
        read.is_some() && header,
        format!("{:?}", read.map(|r| r.len())),
    ));
    let opened = File::open(TCP).map(drop);
    lines.push(helper::line(
        "reopen",
        opened
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(Errno::ACCESS.raw_os_error())),
        format!("{opened:?}"),
    ));
    let tcp = || {
        rustix::net::socket(rustix::net::AddressFamily::INET, rustix::net::SocketType::STREAM, None)
    };
    let local = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let bind = tcp().and_then(|socket| rustix::net::bind(&socket, &local));
    lines.push(helper::line("bind", bind == Err(Errno::PERM), format!("{bind:?}")));
    let listen = tcp().and_then(|socket| rustix::net::listen(&socket, 1));
    lines.push(helper::line("listen", listen == Err(Errno::PERM), format!("{listen:?}")));
    lines.push(helper::exec_line());
    // NOTE: port 1 of the job's own loopback: nothing listens there, so a connect that
    // the domain lets through is refused by the kernel, never by Landlock.
    let connect = TcpStream::connect((Ipv4Addr::LOCALHOST, 1)).map(drop);
    let refused = connect
        .as_ref()
        .is_err_and(|error| error.raw_os_error() == Some(Errno::CONNREFUSED.raw_os_error()));
    lines.push(helper::line("connect", refused, format!("{connect:?}")));
    drop((streams, tables));
    if helper::print_lines(&lines) {
        Ok(())
    } else {
        Err(SbxError::HelperStart {
            helper: Helper::Forwarder,
            problem: "a check of the forwarder's own sandbox failed",
        })
    }
}

#[cfg(test)]
mod tests;
