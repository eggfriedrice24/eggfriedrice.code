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
//! hand a test the side of a socket peer by pid ([`PeerProbe`]). `just install` refuses an `efrd` built with the feature
//! (`efrd --test-seams` prints `on`).

use std::collections::BTreeMap;
use std::path::Path;
#[cfg(feature = "test-sandbox-fake")]
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use efr_protocol::SandboxStatus;

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
}
