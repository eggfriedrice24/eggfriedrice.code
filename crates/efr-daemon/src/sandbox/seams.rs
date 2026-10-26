//! The test seams of the `auto` sandbox (efr's auto spec, section 16.1), behind the
//! cargo feature `test-sandbox-fake`, the one place of this crate besides `screens.rs`
//! and `shells.rs` where a feature cfg may appear.
//!
//! The real paths refuse a fake: the probe refuses a bwrap that root does not own, and
//! the launcher applies Landlock at a hard requirement. So the fake path has its own
//! seams, and a build without the feature has none of them: [`Deps`] can name the
//! launcher to copy, which an in-process daemon cannot find next to itself, and can
//! replace the probe's result. For background jobs it can give every job a fixed
//! environment, so a test needs no hidden zsh that writes its `env` file, and it can
//! hand a test the side of a socket peer by pid ([`PeerProbe`]), and it can name the
//! program that measures the host's mounts, and it can say that efrd runs as root, so a
//! test sees full mode refused without being root. `just install` refuses an `efrd`
//! built with the feature (`efrd --test-seams` prints `on`).
//!
//! The proxy (phase D2) gets its resolver and connector here ([`Seams::proxy_deps`]):
//! a test build never reaches the internet. With a table of test names and the server
//! behind each, a name resolves to its server's loopback address and the connector
//! reaches only those servers; with no table, every name and every address is refused.
//! The pacman files of `@pacman-mirrors` are `/dev/null` in a test build, so a test
//! daemon never reads the machine's mirror list.
//!
//! The filtered bus (phase E, DE14): in a test build both buses come only from here,
//! for the bus proxy's upstream and for an approved whole bus alike. With no seam the
//! system bus is a path that cannot exist and there is no session bus, so a test daemon
//! never reaches the machine's buses. A test names its fake `xdg-dbus-proxy` here too,
//! because an in-process daemon finds no trusted program on a `PATH`.
//!
//! Job port forwards: efrd binds a host port for each forwarded port of a job through
//! [`HostPorts`]. A test build binds through [`TestHostPorts`], which binds a free port
//! of `127.0.0.1` for each port that a job asks for, so a test never takes a fixed port
//! of the machine. Only a test that asks for the system seam
//! (`Deps::with_system_host_ports`) and the `efrd` of the smoke bind the real ports.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
#[cfg(feature = "test-sandbox-fake")]
use std::net::SocketAddr;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV6};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_protocol::SandboxStatus;
use efr_proxy::Limits;
use efr_sandbox::{TCP_LISTEN, TcpRow};

use crate::jobs::forward::SystemHostPorts;

#[cfg(feature = "test-sandbox-fake")]
use crate::Deps;

/// True in a build with the test seams.
pub const TEST_SEAMS: bool = cfg!(feature = "test-sandbox-fake");

/// What a test put in place of the real launcher and probe. Empty without the feature.
#[derive(Debug, Clone, Default)]
pub(crate) struct Seams {
    /// The `efr-sbx` to copy.
    #[cfg(feature = "test-sandbox-fake")]
    launcher: Option<PathBuf>,
    /// The probe's result.
    #[cfg(feature = "test-sandbox-fake")]
    probe: Option<SandboxStatus>,
    /// The dir of the fake `crontab` and `loginctl` that efrd runs for the state copies.
    #[cfg(feature = "test-sandbox-fake")]
    trusted_programs: Option<PathBuf>,
    /// The environment of every job, in place of the hidden shell's `env` file.
    #[cfg(feature = "test-sandbox-fake")]
    job_env: Option<BTreeMap<String, String>>,
    /// Where a test asks for the side of a socket peer.
    #[cfg(feature = "test-sandbox-fake")]
    peer_probe: Option<PeerProbe>,
    /// The program that measures the host's mounts, in place of the launcher's copy.
    #[cfg(feature = "test-sandbox-fake")]
    statvfs_helper: Option<PathBuf>,
    /// The test names of the proxy and the server behind each.
    #[cfg(feature = "test-sandbox-fake")]
    proxy_hosts: Option<BTreeMap<String, SocketAddr>>,
    /// The test's own system bus socket.
    #[cfg(feature = "test-sandbox-fake")]
    system_bus: Option<PathBuf>,
    /// The test's own session bus socket.
    #[cfg(feature = "test-sandbox-fake")]
    session_bus: Option<PathBuf>,
    /// The test's own `xdg-dbus-proxy`.
    #[cfg(feature = "test-sandbox-fake")]
    bus_proxy_program: Option<PathBuf>,
    /// Whether efrd runs as root, in place of its effective uid (full mode, DF27).
    #[cfg(feature = "test-sandbox-fake")]
    runs_as_root: Option<bool>,
    /// The host ports of the job port forwards: a test's map, or the system's.
    #[cfg(feature = "test-sandbox-fake")]
    host_ports: Option<HostPortsSeam>,
    /// Where the remote listener binds in place of the tailnet address.
    #[cfg(feature = "test-sandbox-fake")]
    remote_listen: Option<std::net::SocketAddrV4>,
}

/// The host ports of a test build.
#[cfg(feature = "test-sandbox-fake")]
#[derive(Debug, Clone)]
enum HostPortsSeam {
    /// A free port of `127.0.0.1` for each port that a job asks for.
    Test(TestHostPorts),
    /// The real ports of the machine.
    System,
}

/// The host side of the job port forwards (DL8, DL9): the bind of `127.0.0.1:<port>`
/// and the host's tcp tables.
pub(crate) trait HostPorts: Send + Sync {
    /// Binds `127.0.0.1:port` with no `SO_REUSEPORT`, ready for tokio. The test seam
    /// binds `127.0.0.1:0` and remembers which port stands for `port`. It blocks.
    fn bind(&self, port: u16) -> io::Result<std::net::TcpListener>;
    /// The host's `tcp` and `tcp6` rows. It blocks; async code calls it on the blocking
    /// pool.
    fn rows(&self) -> io::Result<(Vec<TcpRow>, Vec<TcpRow>)>;
    /// The uid that efrd runs as, as the rows show it.
    fn own_uid(&self) -> u32;
}

/// The host ports of a test daemon: each bind of a port takes a free port of
/// `127.0.0.1` in its place, so a test never binds a fixed port of the machine. The
/// rows are the machine's own, so the peer check of a test connection reads the real
/// table; a test can make a port count as in use, add a listener on `::1`, and show
/// another uid in every row.
#[derive(Debug, Clone, Default)]
pub struct TestHostPorts(Arc<Mutex<TestPorts>>);

#[derive(Debug, Default)]
struct TestPorts {
    /// The free port that stands for each port that a job asked for.
    real: BTreeMap<u16, u16>,
    /// The ports whose bind fails with `AddrInUse`.
    in_use: BTreeSet<u16>,
    /// The ports with a `LISTEN` row on `::1` in the `tcp6` rows.
    v6: BTreeSet<u16>,
    /// The uid of every row, in place of the real one.
    uid: Option<u32>,
}

impl TestHostPorts {
    /// A map with no port yet.
    pub fn new() -> TestHostPorts {
        TestHostPorts::default()
    }

    fn lock(&self) -> MutexGuard<'_, TestPorts> {
        // NOTE: each change is whole, so a poisoned lock still holds a usable map.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The free port of `127.0.0.1` that stands for `port`, once efrd bound it.
    pub fn real_port(&self, port: u16) -> Option<u16> {
        self.lock().real.get(&port).copied()
    }

    /// Makes the bind of `port` fail as for a port that another program holds.
    pub fn set_in_use(&self, port: u16) {
        self.lock().in_use.insert(port);
    }

    /// Adds a `LISTEN` row on `[::1]:port` to the host's `tcp6` rows, as for a server
    /// of the user that listens on IPv6 only.
    pub fn set_v6_listen(&self, port: u16) {
        self.lock().v6.insert(port);
    }

    /// Shows `uid` as the owner of every row of the host, as for a peer of another
    /// user.
    pub fn set_peer_uid(&self, uid: u32) {
        self.lock().uid = Some(uid);
    }
}

impl HostPorts for TestHostPorts {
    fn bind(&self, port: u16) -> io::Result<std::net::TcpListener> {
        if self.lock().in_use.contains(&port) {
            return Err(io::Error::from(io::ErrorKind::AddrInUse));
        }
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let real = listener.local_addr()?.port();
        self.lock().real.insert(port, real);
        Ok(listener)
    }

    fn rows(&self) -> io::Result<(Vec<TcpRow>, Vec<TcpRow>)> {
        let (mut v4, mut v6) = SystemHostPorts.rows()?;
        let ports = self.lock();
        if let Some(uid) = ports.uid {
            for row in v4.iter_mut().chain(v6.iter_mut()) {
                row.uid = uid;
            }
        }
        let uid = ports.uid.unwrap_or_else(|| SystemHostPorts.own_uid());
        for port in &ports.v6 {
            v6.push(TcpRow {
                state: TCP_LISTEN,
                local: SocketAddrV6::new(Ipv6Addr::LOCALHOST, *port, 0, 0).into(),
                remote: SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0).into(),
                uid,
            });
        }
        Ok((v4, v6))
    }

    fn own_uid(&self) -> u32 {
        SystemHostPorts.own_uid()
    }
}

/// The system bus of a test build without the seam: a path below `/proc`, where no
/// process can make a file, so it never exists.
#[cfg(feature = "test-sandbox-fake")]
const NO_TEST_SYSTEM_BUS: &str = "/proc/efr-test-no-system-bus";

/// The side of a socket peer as a test reads it: `user`, `model_side` or `unknown`.
type PeerSideFn = dyn Fn(u32) -> &'static str + Send + Sync;

/// Where a test asks efrd for the side of the socket peer with a pid, as
/// `methods.rs` decides it: through the hidden shells and the jobs' launchers and
/// sessions. Only a build with the feature `test-sandbox-fake` fills one.
#[derive(Clone, Default)]
pub struct PeerProbe(Arc<Mutex<Option<Arc<PeerSideFn>>>>);

impl std::fmt::Debug for PeerProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PeerProbe")
    }
}

impl PeerProbe {
    /// A probe that efrd fills at its start.
    pub fn new() -> PeerProbe {
        PeerProbe::default()
    }

    /// The side of the peer `pid`: `user`, `model_side` or `unknown`; `None` before
    /// efrd started. It reads `/proc`, so it blocks for a few small reads.
    pub fn side(&self, pid: u32) -> Option<&'static str> {
        let side = self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()?;
        Some(side(pid))
    }

    #[cfg(feature = "test-sandbox-fake")]
    fn set(&self, side: Arc<PeerSideFn>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(side);
    }
}

#[cfg(feature = "test-sandbox-fake")]
impl Seams {
    /// The launcher a test named, which wins over the one next to efrd.
    pub(crate) fn launcher(&self) -> Option<&Path> {
        self.launcher.as_deref()
    }

    /// The probe's result that a test set; the launcher's probe then never runs.
    pub(crate) fn probe(&self) -> Option<&SandboxStatus> {
        self.probe.as_ref()
    }

    /// The dir of fake programs that a test named for the state copies, in place of
    /// the real ones on efrd's `PATH`.
    pub(crate) fn trusted_programs(&self) -> Option<&Path> {
        self.trusted_programs.as_deref()
    }

    /// The environment that a test gives every job.
    pub(crate) fn job_env(&self) -> Option<BTreeMap<String, String>> {
        self.job_env.clone()
    }

    /// Lets a test's probe ask for the side of a peer through `side`.
    pub(crate) fn fill_peer_probe(&self, side: Arc<PeerSideFn>) {
        if let Some(probe) = &self.peer_probe {
            probe.set(side);
        }
    }

    /// The `statvfs` helper that a test named, in place of the launcher's copy.
    pub(crate) fn statvfs_helper(&self) -> Option<&Path> {
        self.statvfs_helper.as_deref()
    }

    /// The proxy's deps in a test build: the table resolver and connector of the test
    /// names (from [`Deps::with_proxy_hosts`], else `EFR_TEST_PROXY_HOSTS` read by
    /// [`Seams::with_process_env`]), no own address and the loopback guard. With no
    /// table, every name and every address is refused, so no test daemon reaches the
    /// internet.
    pub(crate) fn proxy_deps(&self, limits: Limits) -> Option<efr_proxy::Deps> {
        let table = Arc::new(self.proxy_hosts.clone().unwrap_or_default());
        Some(efr_proxy::Deps {
            resolver: Arc::new(TableResolver(Arc::clone(&table))),
            connector: Arc::new(TableConnector(table)),
            own: Arc::new(NoAddresses),
            guard: efr_proxy::Guard::with_loopback(),
            limits,
        })
    }

    /// The pacman files of `@pacman-mirrors`: `/dev/null` in a test build, so a test
    /// daemon lists no mirror of the machine.
    pub(crate) fn pacman_files(&self) -> [PathBuf; 2] {
        [PathBuf::from("/dev/null"), PathBuf::from("/dev/null")]
    }

    /// The system bus of a test build: the test's own socket, or a path that never
    /// exists (DE14).
    pub(crate) fn system_bus(&self) -> PathBuf {
        self.system_bus.clone().unwrap_or_else(|| PathBuf::from(NO_TEST_SYSTEM_BUS))
    }

    /// The session bus of a test build: the test's own socket, never the one of the
    /// shells' environment `_host` (DE14).
    pub(crate) fn session_bus(&self, _host: Option<PathBuf>) -> Option<PathBuf> {
        self.session_bus.clone()
    }

    /// The `xdg-dbus-proxy` that a test named, in place of a trusted program.
    pub(crate) fn bus_proxy_program(&self) -> Option<&Path> {
        self.bus_proxy_program.as_deref()
    }

    /// Whether efrd runs as root, as a test said, in place of its effective uid.
    pub(crate) fn runs_as_root(&self) -> Option<bool> {
        self.runs_as_root
    }

    /// Where the remote listener binds, as a test said, in place of the tailnet
    /// address that tailscaled names.
    pub(crate) fn remote_listen(&self) -> Option<std::net::SocketAddrV4> {
        self.remote_listen
    }

    /// The host ports of the job port forwards in a test build: the test's map, the
    /// system's when a test or the smoke's `efrd` asked for it, else a map of its own,
    /// so a test daemon never binds a fixed port of the machine.
    pub(crate) fn host_ports(&self) -> Arc<dyn HostPorts> {
        match &self.host_ports {
            Some(HostPortsSeam::Test(ports)) => Arc::new(ports.clone()),
            Some(HostPortsSeam::System) => Arc::new(SystemHostPorts),
            None => Arc::new(TestHostPorts::new()),
        }
    }

    /// These seams with the test names of `EFR_TEST_PROXY_HOSTS` and the test buses of
    /// `EFR_TEST_SYSTEM_BUS` and `EFR_TEST_SESSION_BUS`, for the `efrd` of the smoke. A
    /// value that does not parse names nothing and costs a warning.
    pub(crate) fn with_process_env(mut self) -> Self {
        use efr_stdx::env::Var;
        // NOTE: the smoke's `efrd` forwards the real ports, so curl and a browser reach
        // a job's server; only an in-process test daemon gets a map.
        if self.host_ports.is_none() {
            self.host_ports = Some(HostPortsSeam::System);
        }
        match efr_stdx::env::var(Var::TestProxyHosts) {
            Ok(Some(value)) => match parse_proxy_hosts(&value) {
                Some(hosts) => self.proxy_hosts = Some(hosts),
                None => tracing::warn!("EFR_TEST_PROXY_HOSTS does not parse; it names no host"),
            },
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "EFR_TEST_PROXY_HOSTS cannot be read"),
        }
        for (var, slot) in [
            (Var::TestSystemBus, &mut self.system_bus),
            (Var::TestSessionBus, &mut self.session_bus),
        ] {
            match efr_stdx::env::var(var) {
                Ok(Some(value)) if Path::new(&value).is_absolute() => {
                    *slot = Some(PathBuf::from(value));
                }
                Ok(Some(_)) => {
                    tracing::warn!(%var, "the test bus is not an absolute path; it names no bus")
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%var, %error, "the test bus cannot be read"),
            }
        }
        self
    }
}

/// `name=address:port,...` as a table; `None` when an entry does not parse or names
/// no loopback address.
#[cfg(feature = "test-sandbox-fake")]
pub(crate) fn parse_proxy_hosts(value: &str) -> Option<BTreeMap<String, SocketAddr>> {
    let mut hosts = BTreeMap::new();
    for entry in value.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
        let (name, server) = entry.split_once('=')?;
        let server: SocketAddr = server.trim().parse().ok()?;
        if !server.ip().is_loopback() {
            return None;
        }
        hosts.insert(name.trim().to_ascii_lowercase(), server);
    }
    Some(hosts)
}

/// A test name to the loopback address of its server; every other name is unknown.
#[cfg(feature = "test-sandbox-fake")]
struct TableResolver(Arc<BTreeMap<String, SocketAddr>>);

#[cfg(feature = "test-sandbox-fake")]
impl efr_proxy::Resolver for TableResolver {
    fn resolve<'a>(&'a self, host: &'a str) -> efr_proxy::Resolving<'a> {
        let found = self.0.get(host).map(|server| vec![server.ip()]);
        Box::pin(async move {
            found.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such test name"))
        })
    }
}

/// The loopback address of a test server on port 443 or 80 to the server's own port;
/// every other address is refused, so no byte leaves the machine.
#[cfg(feature = "test-sandbox-fake")]
struct TableConnector(Arc<BTreeMap<String, SocketAddr>>);

#[cfg(feature = "test-sandbox-fake")]
impl efr_proxy::Connector for TableConnector {
    fn connect(&self, addr: SocketAddr) -> efr_proxy::Connecting<'_> {
        let server = self
            .0
            .values()
            .find(|server| server.ip() == addr.ip() && matches!(addr.port(), 443 | 80))
            .copied()
            .filter(|server| server.ip().is_loopback());
        Box::pin(async move {
            let Some(server) = server else {
                return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "no test server"));
            };
            tokio::net::TcpStream::connect(server).await
        })
    }
}

/// No own address: the test names resolve to loopback addresses only.
#[cfg(feature = "test-sandbox-fake")]
struct NoAddresses;

#[cfg(feature = "test-sandbox-fake")]
impl efr_proxy::OwnAddresses for NoAddresses {
    fn addresses(&self) -> Vec<(std::net::IpAddr, u8)> {
        Vec::new()
    }
}

#[cfg(not(feature = "test-sandbox-fake"))]
impl Seams {
    /// Without the feature no test names a launcher.
    pub(crate) fn launcher(&self) -> Option<&Path> {
        None
    }

    /// Without the feature the probe always runs.
    pub(crate) fn probe(&self) -> Option<&SandboxStatus> {
        None
    }

    /// Without the feature efrd finds `crontab` and `loginctl` on its own `PATH` only.
    pub(crate) fn trusted_programs(&self) -> Option<&Path> {
        None
    }

    /// Without the feature every job gets the hidden shell's environment.
    pub(crate) fn job_env(&self) -> Option<BTreeMap<String, String>> {
        None
    }

    /// Without the feature no test asks for the side of a peer.
    pub(crate) fn fill_peer_probe(&self, _side: Arc<PeerSideFn>) {}

    /// Without the feature the launcher's copy measures the mounts.
    pub(crate) fn statvfs_helper(&self) -> Option<&Path> {
        None
    }

    /// Without the feature efrd builds the system resolver, connector and addresses
    /// with the strict guard.
    pub(crate) fn proxy_deps(&self, _limits: Limits) -> Option<efr_proxy::Deps> {
        None
    }

    /// Without the feature `@pacman-mirrors` reads the machine's pacman files (DP4):
    /// `pacman.conf`, whose `Include =` lines name more files, and the mirror list.
    pub(crate) fn pacman_files(&self) -> [PathBuf; 2] {
        [PathBuf::from("/etc/pacman.conf"), PathBuf::from("/etc/pacman.d/mirrorlist")]
    }

    /// Without the feature no variable names test hosts or buses.
    pub(crate) fn with_process_env(self) -> Self {
        self
    }

    /// Without the feature the system bus is the machine's.
    pub(crate) fn system_bus(&self) -> PathBuf {
        PathBuf::from(efr_sandbox::SYSTEM_BUS_SOCKET)
    }

    /// Without the feature the session bus is the one of the shells' environment.
    pub(crate) fn session_bus(&self, host: Option<PathBuf>) -> Option<PathBuf> {
        host
    }

    /// Without the feature efrd runs only the trusted `xdg-dbus-proxy` of its `PATH`.
    pub(crate) fn bus_proxy_program(&self) -> Option<&Path> {
        None
    }

    /// Without the feature efrd reads its effective uid.
    pub(crate) fn runs_as_root(&self) -> Option<bool> {
        None
    }

    /// Without the feature the remote listener binds only the tailnet address.
    pub(crate) fn remote_listen(&self) -> Option<std::net::SocketAddrV4> {
        None
    }

    /// Without the feature efrd binds the real ports of the machine.
    pub(crate) fn host_ports(&self) -> Arc<dyn HostPorts> {
        Arc::new(SystemHostPorts)
    }
}

#[cfg(feature = "test-sandbox-fake")]
impl Deps {
    /// Copies `launcher` as the sandbox's launcher, instead of the `efr-sbx` next to
    /// efrd. Only with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_sandbox_launcher(mut self, launcher: impl Into<PathBuf>) -> Self {
        self.seams.launcher = Some(launcher.into());
        self
    }

    /// Uses `status` as every probe's result, so the launcher's probe never runs. Only
    /// with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_probe_override(mut self, status: SandboxStatus) -> Self {
        self.seams.probe = Some(status);
        self
    }

    /// Runs the fake `crontab` and `loginctl` of `dir` for the state copies of cron and
    /// linger, which a test owns, instead of the root-owned ones on efrd's `PATH`. Only
    /// with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_trusted_program_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.seams.trusted_programs = Some(dir.into());
        self
    }

    /// Gives every background job `env` as its environment, in place of the `env` file
    /// of the hidden shell, so a test with a fake PTY holder starts jobs. Only with the
    /// feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_job_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.seams.job_env = Some(env);
        self
    }

    /// Lets `probe` ask efrd for the side of a socket peer by pid. Only with the
    /// feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_peer_probe(mut self, probe: PeerProbe) -> Self {
        self.seams.peer_probe = Some(probe);
        self
    }

    /// Runs `helper statvfs` to measure the host's mounts for the mount view, in place
    /// of the launcher's copy: a script that prints fixed answers, or one that blocks.
    /// Only with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_statvfs_helper(mut self, helper: impl Into<PathBuf>) -> Self {
        self.seams.statvfs_helper = Some(helper.into());
        self
    }

    /// Sends the proxy's connections for each test name of `hosts` to the server
    /// behind it, a loopback address (`127.0.0.2:41001`): the name resolves to that
    /// address, and port 443 or 80 of it reaches the server. Every other name is
    /// refused. Only with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_proxy_hosts(mut self, hosts: BTreeMap<String, SocketAddr>) -> Self {
        self.seams.proxy_hosts = Some(hosts);
        self
    }

    /// Uses `socket` as the system bus: the upstream of the bus filter and the socket
    /// of an approved whole bus. Without it a test daemon has no system bus. Only with
    /// the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_system_bus(mut self, socket: impl Into<PathBuf>) -> Self {
        self.seams.system_bus = Some(socket.into());
        self
    }

    /// Uses `socket` as the session bus, in place of the one of the shells'
    /// environment. Without it a test daemon has no session bus. Only with the feature
    /// `test-sandbox-fake`.
    #[must_use]
    pub fn with_session_bus(mut self, socket: impl Into<PathBuf>) -> Self {
        self.seams.session_bus = Some(socket.into());
        self
    }

    /// Runs `program` as `xdg-dbus-proxy`, a test's fake that the test owns, in place
    /// of the trusted program on efrd's `PATH`. Only with the feature
    /// `test-sandbox-fake`.
    #[must_use]
    pub fn with_bus_proxy_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.seams.bus_proxy_program = Some(program.into());
        self
    }

    /// Says whether efrd runs as root, in place of its effective uid, so a test sees
    /// full mode refused as it is for root (`FullRefusal::Root`) without being root.
    /// Only with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_runs_as_root(mut self, root: bool) -> Self {
        self.seams.runs_as_root = Some(root);
        self
    }

    /// Binds the host port of each job port forward through `ports`, a map of free
    /// ports that the test reads. Without it a test daemon has a map of its own. Only
    /// with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_test_host_ports(mut self, ports: TestHostPorts) -> Self {
        self.seams.host_ports = Some(HostPortsSeam::Test(ports));
        self
    }

    /// Binds the real port of the machine for each job port forward, as efrd does
    /// without the test seams: for the one test of the real bind. Only with the feature
    /// `test-sandbox-fake`.
    #[must_use]
    pub fn with_system_host_ports(mut self) -> Self {
        self.seams.host_ports = Some(HostPortsSeam::System);
        self
    }

    /// Binds the remote listener at `addr`, such as `127.0.0.1:0`, in place of the
    /// tailnet address that tailscaled names. The whois of the test's tailscaled still
    /// decides each peer; without the feature the listener never binds anything but the
    /// tailnet address. Only with the feature `test-sandbox-fake`.
    #[must_use]
    pub fn with_remote_listen(mut self, addr: std::net::SocketAddrV4) -> Self {
        self.seams.remote_listen = Some(addr);
        self
    }
}
