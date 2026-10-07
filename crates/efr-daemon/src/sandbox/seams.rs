//! The test seams of the `auto` sandbox (efr's auto spec, section 16.1), behind the
//! cargo feature `test-sandbox-fake`, the one place of this crate besides `screens.rs`
//! and `shells.rs` where a feature cfg may appear.
//!
//! The real paths refuse a fake: the probe refuses a bwrap that root does not own, and
//! the launcher applies Landlock at a hard requirement. So the fake path has its own
//! seams, and a build without the feature has none of them: [`Deps`] can name the
//! launcher to copy, which an in-process daemon cannot find next to itself, and can
//! replace the probe's result. `just install` refuses an `efrd` built with the feature
//! (`efrd --test-seams` prints `on`).

use std::path::Path;
#[cfg(feature = "test-sandbox-fake")]
use std::path::PathBuf;

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
}
