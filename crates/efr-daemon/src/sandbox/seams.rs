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
//! program that measures the host's mounts. `just install` refuses an `efrd` built with the feature
//! (`efrd --test-seams` prints `on`).
//!
//! The proxy (phase D2) gets its resolver and connector here ([`Seams::proxy_deps`]):
//! a test build never reaches the internet. With a table of test names and the server
//! behind each, a name resolves to its server's loopback address and the connector
//! reaches only those servers; with no table, every name and every address is refused.
//! The pacman files of `@pacman-mirrors` are `/dev/null` in a test build, so a test
//! daemon never reads the machine's mirror list.

use std::collections::BTreeMap;
#[cfg(feature = "test-sandbox-fake")]
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use efr_protocol::SandboxStatus;
use efr_proxy::Limits;

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
}

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

    /// These seams with the test names of `EFR_TEST_PROXY_HOSTS`, for the `efrd` of the
    /// smoke. A value that does not parse names no host and costs a warning.
    pub(crate) fn with_process_env(mut self) -> Self {
        match efr_stdx::env::var(efr_stdx::env::Var::TestProxyHosts) {
            Ok(Some(value)) => match parse_proxy_hosts(&value) {
                Some(hosts) => self.proxy_hosts = Some(hosts),
                None => tracing::warn!("EFR_TEST_PROXY_HOSTS does not parse; it names no host"),
            },
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "EFR_TEST_PROXY_HOSTS cannot be read"),
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
            found.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no such test name")
            })
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
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "no test server",
                ));
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

    /// Without the feature no variable names test hosts.
    pub(crate) fn with_process_env(self) -> Self {
        self
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
}
