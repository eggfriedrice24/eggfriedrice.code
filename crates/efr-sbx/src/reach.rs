//! `efr-sbx reach`: the reach helper of a contained call.
//!
//! A contained call has a network namespace of its own with loopback only, so it cannot
//! reach the forwarded ports of its conversation's background jobs. The reach helper
//! listens on `127.0.0.1` and `::1` of the call's namespace for each such port, and
//! sends each connection over the call's reach socket (`$XRT/efr-reach.sock`) to efrd,
//! which opens it in the job. The child of the call cannot reach the socket itself: its
//! Landlock domain has no `RESOLVE_UNIX` rule for it.
//!
//! The inner stage starts it as the forwarder, with the starter and the setup of
//! `helper.rs`, and binds nothing for it:
//!
//! 1. The starter, `efr-sbx reach --socket P --port N ...`, runs the server, `efr-sbx
//!    reach --serve --ready-fd 1 ...`, and waits for its ready byte.
//! 2. The server closes every descriptor from 3 up, makes a session of its own, names
//!    itself `efr-reach`, finds the socket, makes itself not dumpable, and applies its
//!    own Landlock domain (`LandlockPolicy::reach`: no file right, `RESOLVE_UNIX` on the
//!    socket only, the TCP connect right handled with no rule) and seccomp profile
//!    (`SeccompProfile::reach`: only Unix, IPv4 and IPv6 sockets; no `execve`). Then it
//!    listens on both addresses for each port of the start, and only then writes `R`.
//!    So the ports of the start are the helper's before the child runs, and a `bind` of
//!    the child gets `EADDRINUSE`, as on the host where efrd holds them.
//! 3. It connects to the socket and sends `Hello` with `REACH_VERSION`, then one `Bound`
//!    for each listen of the start: the control connection. It reads efrd's `Reach` and
//!    `Unreach` frames there, one address and one port each, and answers each `Reach`
//!    with a `Bound`: status 0 when it listens, else the errno. A port that the child
//!    holds already stays the child's (`EADDRINUSE`); the helper does not try again.
//! 4. For each connection of the call's processes to a listen, a thread connects to the
//!    socket, sends `Connect` with the address and the port, and copies bytes both ways.
//!    efrd closes that connection with no byte when no job's server answers; the helper
//!    then resets the client's connection (`SO_LINGER` 0), so the client sees a reset
//!    and not an empty answer. A server that closes with no byte looks the same.
//!
//! The helper refuses in its own code a port below 1024, the bridge's 3128 and every
//! address but `127.0.0.1` and `::1` (`Bound` with `EPERM`), more than
//! [`MAX_DAEMON_FORWARDS`] ports (`EBUSY`) and more than [`MAX_REACH_CONNECTIONS`] open
//! connections (a reset). It never opens a TCP connection: its Landlock domain refuses
//! every TCP connect. It keeps `bind` and `listen`, because ports come during the call
//! and Landlock takes no rule after its domain starts; a bind in the call's namespace
//! reaches only the call, which can bind the same ports itself.
//!
//! A reach helper that does not start never fails the call: the inner stage writes
//! nothing about it, and efrd sees that no `Hello` came. The helper ends when efrd closes
//! the control connection, when efrd sends a frame that is not a valid `Reach` or
//! `Unreach`, and with the call's namespace.
//!
//! `--check` runs the server's setup and checks its domain from inside a call: not
//! dumpable, a write, a file open, a TCP connect and `execve` refused, a listen still
//! works.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::thread::JoinHandle;
use std::time::Duration;

use efr_sandbox::{
    BRIDGE_PORT, FRAME_LEN, Frame, FrameKind, InnerReach, LandlockPolicy, Listen,
    MAX_DAEMON_FORWARDS, MAX_REACH_CONNECTIONS, MAX_REACH_PORTS, MIN_PORT, SeccompProfile,
};
use rustix::io::Errno;

use crate::error::SbxError;
use crate::helper::{self, Confinement, HalfClose, Helper, Slot, Step, Take};
use crate::os;
use crate::self_test::socket_path;

/// The name of the server's process.
pub(crate) const REACH_NAME: &str = "efr-reach";

/// The addresses of the call's loopback where the helper listens.
const ADDRESSES: [IpAddr; 2] = [IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)];

/// The status of a `Bound` for an address or a port that the helper never binds:
/// `EPERM`.
pub(crate) const STATUS_REFUSED: u8 = 1;

/// The status of a `Bound` when the helper holds [`MAX_DAEMON_FORWARDS`] ports: `EBUSY`.
pub(crate) const STATUS_BUSY: u8 = 16;

/// The status of a `Bound` for a bind that failed with no errno: `EIO`.
const STATUS_FAILED: u8 = 5;

/// The most bytes of the first read of efrd's answer.
const FIRST_READ: usize = 16 * 1024;

/// The pause after an accept that failed for a reason other than the end of the
/// listener, such as no free descriptor for a moment.
const ACCEPT_PAUSE: Duration = Duration::from_millis(50);

/// The arguments of `efr-sbx reach`.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct ReachArgs {
    /// The call's reach socket inside, `$XRT/efr-reach.sock`.
    #[arg(long)]
    pub(crate) socket: PathBuf,
    /// A port that the helper listens on at `127.0.0.1` and `::1` before the call's child
    /// runs; more than one may come.
    #[arg(long = "port")]
    pub(crate) ports: Vec<u16>,
    /// Run the server: the setup, the ports of the start, the ready byte, then the
    /// control connection and the connections.
    #[arg(long)]
    pub(crate) serve: bool,
    /// The server's ready pipe: 1.
    #[arg(long)]
    pub(crate) ready_fd: Option<i32>,
    /// Run the server's setup and check its domain; exit 0 when every check passes.
    #[arg(long)]
    pub(crate) check: bool,
    /// With `--check`: a directory that the call may write; a new file there must fail
    /// with EACCES.
    #[arg(long, requires = "check")]
    pub(crate) write: Option<PathBuf>,
    /// End the server's setup before this step, for the tests.
    #[arg(long, value_enum)]
    pub(crate) fail_setup: Option<Step>,
}

/// The starter's arguments for the inner stage's `reach`: `efr-sbx reach --socket P`
/// and one `--port N` for each port of the start.
pub(crate) fn argv(reach: &InnerReach) -> Vec<OsString> {
    let mut argv: Vec<OsString> =
        vec!["reach".into(), "--socket".into(), reach.socket.clone().into_os_string()];
    for port in &reach.ports {
        argv.extend(["--port".into(), port.to_string().into()]);
    }
    argv
}

/// Runs the starter, the server or the check.
pub(crate) fn main(args: &ReachArgs) -> ExitCode {
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
            let _ = writeln!(io::stderr(), "efr-sbx reach: {}", error.chain());
            ExitCode::FAILURE
        }
    }
}

/// The starter: checks the ports, runs the server and waits for its ready byte.
fn start(args: &ReachArgs) -> Result<(), SbxError> {
    check_ports(&args.ports)?;
    let reach = InnerReach { socket: args.socket.clone(), ports: args.ports.clone(), proxy: false };
    let mut argv = argv(&reach);
    argv.extend(["--serve".into(), "--ready-fd".into(), "1".into()]);
    if let Some(step) = args.fail_setup {
        argv.extend(["--fail-setup".into(), step.word().into()]);
    }
    helper::start(Helper::Reach, argv, None)
}

/// True for a port that the helper never binds: below [`MIN_PORT`], or the bridge's
/// [`BRIDGE_PORT`], also in a call with no proxy plan, as efrd's spec check and the frame
/// decoder.
pub(crate) fn refused_port(port: u16) -> bool {
    port < MIN_PORT || port == BRIDGE_PORT
}

/// True for a listen that the helper never makes: a refused port, or an address other
/// than `127.0.0.1` and `::1`.
pub(crate) fn refused(listen: Listen) -> bool {
    refused_port(listen.port) || !ADDRESSES.contains(&listen.addr)
}

/// Refuses the ports of a start that efrd's spec check would refuse: more than
/// [`MAX_REACH_PORTS`], a refused port, or a port twice.
pub(crate) fn check_ports(ports: &[u16]) -> Result<(), SbxError> {
    let refuse = |problem| Err(SbxError::HelperStart { helper: Helper::Reach, problem });
    if ports.len() > MAX_REACH_PORTS {
        return refuse("more ports than the reach helper holds");
    }
    for (index, port) in ports.iter().enumerate() {
        if refused_port(*port) {
            return refuse("a port that the reach helper never binds");
        }
        if ports.get(..index).is_some_and(|before| before.contains(port)) {
            return refuse("a port twice");
        }
    }
    Ok(())
}

/// The helper's confinement.
fn confinement(args: &ReachArgs) -> Confinement {
    Confinement {
        helper: Helper::Reach,
        landlock: LandlockPolicy::reach(&args.socket),
        seccomp: SeccompProfile::reach(),
        fail_setup: args.fail_setup,
    }
}

/// The path to connect to, with the descriptor of its dir when the path is too long
/// for a socket address. The helper finds it before its own domain.
fn open_socket(socket: &Path) -> Result<(Option<OwnedFd>, PathBuf), SbxError> {
    socket_path(socket).map_err(|error| SbxError::io("find", socket, error))
}

/// The server: the setup, the ports of the start, the ready byte, then the control
/// connection.
fn serve_main(args: &ReachArgs) -> Result<(), SbxError> {
    check_ports(&args.ports)?;
    let take = Take { listen_fd: None, ready_fd: args.ready_fd };
    let (streams, (_dir, socket)) =
        helper::setup(&confinement(args), Some(take), || open_socket(&args.socket))?;
    let Some(ready) = streams.ready else {
        return Err(SbxError::os("take the ready pipe", io::ErrorKind::InvalidInput.into()));
    };
    let mut listeners = Listeners::new(socket.clone());
    // NOTE: the ports of the start listen before the ready byte, and the inner stage
    // runs the child only after the starter ended: they are the helper's before the
    // child runs.
    let mut bound = Vec::new();
    for port in &args.ports {
        for addr in ADDRESSES {
            let listen = Listen { addr, port: *port };
            bound.push(Frame::bound(listen, listeners.reach(listen)));
        }
    }
    helper::report_ready(ready)?;
    serve(&socket, &mut listeners, &bound)
}

/// Sends `Hello` and the `Bound` frames of the start, then answers efrd's frames until
/// efrd closes the control connection.
fn serve(socket: &Path, listeners: &mut Listeners, bound: &[Frame]) -> Result<(), SbxError> {
    let mut control =
        UnixStream::connect(socket).map_err(|error| SbxError::io("connect to", socket, error))?;
    let send = |control: &mut UnixStream, frame: Frame| {
        control
            .write_all(&frame.encode())
            .map_err(|error| SbxError::os("send a frame of the reach helper", error))
    };
    send(&mut control, Frame::reach_hello())?;
    for frame in bound {
        send(&mut control, *frame)?;
    }
    let mut bytes = [0_u8; FRAME_LEN];
    loop {
        // NOTE: `read_exact` retries an interrupted read itself, so an error here means
        // that efrd closed the control connection: the reach of this call ends.
        if control.read_exact(&mut bytes).is_err() {
            return Ok(());
        }
        let frame = Frame::decode(&bytes)?;
        match frame.kind {
            FrameKind::Reach => {
                let status = listeners.reach(frame.target());
                send(&mut control, Frame::bound(frame.target(), status))?;
            }
            FrameKind::Unreach => listeners.unreach(frame.target()),
            _ => {
                return Err(SbxError::os("read efrd's frame", io::ErrorKind::InvalidData.into()));
            }
        }
    }
}

/// The status of a `Bound` for a bind that failed with `error`.
fn status_of(error: &io::Error) -> u8 {
    error
        .raw_os_error()
        .and_then(|errno| u8::try_from(errno).ok())
        .filter(|errno| *errno != 0)
        .unwrap_or(STATUS_FAILED)
}

/// One listen of the helper and the thread that accepts its connections.
#[derive(Debug)]
struct Held {
    listener: Arc<TcpListener>,
    accept: Option<JoinHandle<()>>,
}

/// The listens of the helper, which only the control connection's thread changes.
#[derive(Debug)]
pub(crate) struct Listeners {
    held: HashMap<Listen, Held>,
    /// The call's reach socket, as the helper connects to it.
    socket: PathBuf,
    /// The open connections, at most [`MAX_REACH_CONNECTIONS`].
    open: Arc<AtomicUsize>,
}

impl Listeners {
    /// No listen yet; each connection goes to `socket`.
    pub(crate) fn new(socket: PathBuf) -> Listeners {
        Listeners { held: HashMap::new(), socket, open: Arc::new(AtomicUsize::new(0)) }
    }

    /// The ports that the helper listens on, on either address.
    fn ports(&self) -> BTreeSet<u16> {
        self.held.keys().map(|listen| listen.port).collect()
    }

    /// Listens on `listen` and accepts its connections on a thread of its own. Returns
    /// the status of its `Bound`: 0 when it listens, also when it listened already, or
    /// the errno.
    pub(crate) fn reach(&mut self, listen: Listen) -> u8 {
        if refused(listen) {
            return STATUS_REFUSED;
        }
        if self.held.contains_key(&listen) {
            return 0;
        }
        let ports = self.ports();
        if !ports.contains(&listen.port) && ports.len() >= MAX_DAEMON_FORWARDS {
            return STATUS_BUSY;
        }
        // NOTE: std sets SO_REUSEADDR on the listener and never SO_REUSEPORT, so a
        // process of the call that holds the port keeps it: the bind fails with
        // EADDRINUSE, and the port stays the call's.
        let listener = match TcpListener::bind(SocketAddr::new(listen.addr, listen.port)) {
            Ok(listener) => Arc::new(listener),
            Err(error) => return status_of(&error),
        };
        let accepting = Arc::clone(&listener);
        let socket = self.socket.clone();
        let open = Arc::clone(&self.open);
        let accept = std::thread::Builder::new()
            .name("efr-reach-acpt".to_owned())
            .stack_size(helper::COPY_STACK)
            .spawn(move || accept(&accepting, listen, &socket, &open));
        match accept {
            Ok(accept) => {
                self.held.insert(listen, Held { listener, accept: Some(accept) });
                0
            }
            Err(error) => status_of(&error),
        }
    }

    /// Closes the listen of `listen`, when the helper holds it. The port is free when
    /// this returns.
    pub(crate) fn unreach(&mut self, listen: Listen) {
        let Some(mut held) = self.held.remove(&listen) else { return };
        // NOTE: a shutdown of a listening socket wakes its accept with EINVAL; the
        // accept thread then ends and drops its copy, and the drop below closes the
        // socket, so a bind of the port works once this returns.
        let _ = rustix::net::shutdown(&*held.listener, rustix::net::Shutdown::Read);
        if let Some(accept) = held.accept.take() {
            let _ = accept.join();
        }
        drop(held);
    }
}

/// Accepts the connections of `listener` until its shutdown.
fn accept(listener: &TcpListener, listen: Listen, socket: &Path, open: &Arc<AtomicUsize>) {
    loop {
        match listener.accept() {
            Ok((tcp, _)) => connection(tcp, listen, socket, open),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.raw_os_error() == Some(Errno::INVAL.raw_os_error()) => return,
            Err(_) => os::sleep(ACCEPT_PAUSE),
        }
    }
}

/// One connection of the call to `listen`: a place, then the route on a thread of its
/// own. With no free place, the client gets a reset.
fn connection(tcp: TcpStream, listen: Listen, socket: &Path, open: &Arc<AtomicUsize>) {
    let Some(slot) = Slot::take(open, MAX_REACH_CONNECTIONS) else {
        reset(tcp);
        return;
    };
    let socket = socket.to_path_buf();
    let _ = helper::spawn_copy(Helper::Reach, move || {
        route(tcp, listen, &socket);
        drop(slot);
    });
}

/// Ends `tcp` with a reset: `SO_LINGER` 0, then the close of the last descriptor.
fn reset(tcp: TcpStream) {
    let _ = rustix::net::sockopt::set_socket_linger(&tcp, Some(Duration::ZERO));
    drop(tcp);
}

/// Sends the connection `tcp` to `listen` to efrd: a new connection to `socket`, the
/// `Connect` frame, then the copies both ways. When efrd closes with no byte, the client
/// gets a reset.
pub(crate) fn route(tcp: TcpStream, listen: Listen, socket: &Path) {
    let Ok(mut unix) = UnixStream::connect(socket) else {
        reset(tcp);
        return;
    };
    if unix.write_all(&Frame::connect(listen).encode()).is_err() {
        reset(tcp);
        return;
    }
    let (Ok(tcp_out), Ok(unix_out)) = (tcp.try_clone(), unix.try_clone()) else {
        reset(tcp);
        return;
    };
    let out =
        helper::spawn_copy(Helper::Reach, move || helper::copy(ClientReads(tcp_out), unix_out));
    let mut first = vec![0_u8; FIRST_READ];
    let read = loop {
        match unix.read(&mut first) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            read => break read,
        }
    };
    let mut tcp = tcp;
    let answered = match read {
        Ok(read) if read > 0 => tcp.write_all(first.get(..read).unwrap_or_default()).is_ok(),
        _ => false,
    };
    if answered {
        helper::copy(unix, tcp);
    } else {
        // NOTE: the other direction's thread holds a copy of the descriptor. The
        // shutdown of the read side ends its copy, so the drop below closes the last
        // descriptor, and SO_LINGER 0 makes that close a reset.
        let _ = rustix::net::sockopt::set_socket_linger(&tcp, Some(Duration::ZERO));
        let _ = tcp.shutdown(Shutdown::Read);
        let _ = unix.shutdown(Shutdown::Both);
    }
    if let Ok(out) = out {
        let _ = out.join();
    }
}

/// The client's connection as the source of the copy to efrd. An error of that copy
/// ends only the read side of the client's connection, never its write side: a
/// shutdown of the write side sends the client an end before `route` resets it, and a
/// client that reads the end first sees an empty answer and not a reset.
pub(crate) struct ClientReads(pub(crate) TcpStream);

impl Read for ClientReads {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl HalfClose for ClientReads {
    fn half_close(&self, _how: Shutdown) {
        let _ = self.0.shutdown(Shutdown::Read);
    }
}

/// `--check`: the server's setup, then the checks of its domain. Prints one JSON line
/// per check, as the self-test does.
fn check(args: &ReachArgs) -> Result<(), SbxError> {
    let setup = helper::setup(&confinement(args), None, || open_socket(&args.socket))?;
    let mut lines = vec![helper::dumpable_line()];
    if let Some(dir) = &args.write {
        lines.push(helper::write_line(dir, ".efr-reach-write"));
    }
    let opened = File::open("/proc/self/status").map(drop);
    lines.push(helper::line("open", is_errno(&opened, Errno::ACCESS), format!("{opened:?}")));
    let listen = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map(drop);
    lines.push(helper::line("listen", listen.is_ok(), format!("{listen:?}")));
    // NOTE: port 1 of the call's own loopback: nothing listens there, so without the
    // domain the kernel refuses the connect with ECONNREFUSED. Landlock's EACCES comes
    // first.
    let connect = TcpStream::connect((Ipv4Addr::LOCALHOST, 1)).map(drop);
    lines.push(helper::line("connect", is_errno(&connect, Errno::ACCESS), format!("{connect:?}")));
    lines.push(helper::exec_line());
    drop(setup);
    if helper::print_lines(&lines) {
        Ok(())
    } else {
        Err(SbxError::HelperStart {
            helper: Helper::Reach,
            problem: "a check of the reach helper's own sandbox failed",
        })
    }
}

/// True when `done` failed with `errno`.
fn is_errno(done: &io::Result<()>, errno: Errno) -> bool {
    done.as_ref().is_err_and(|error| error.raw_os_error() == Some(errno.raw_os_error()))
}

#[cfg(test)]
mod tests;
