//! The hidden shells: `ShellSessions` over the PTY holder, with every byte recorded in
//! the store and every lifecycle notice recorded as an event.
//!
//! This is the only file with a `local-pty` cfg. With the feature, shells run on PTYs
//! that `efr_pty::LocalPtyHolder` opens in this process; without it, and unless the
//! caller injects a holder, every shell start fails with a clear error. The PTY holder
//! milestone deletes the feature and injects a holder that talks to `efr-ptyd`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::ShellSettings;
use efr_holder::PtyHolder;
use efr_permissions::{Policy, Resource};
use efr_shell::{ScreenFactory, ShellConfig, ShellDeps, ShellError, ShellSessions};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;

use crate::DaemonError;

#[cfg(not(feature = "local-pty"))]
mod no_holder;
mod observer;
mod recording;

pub(crate) use observer::{ShellNotices, follow_notices};
pub(crate) use recording::StoreRecording;

/// The shell used when no zsh is on the `PATH`; its runs are delimited by sentinels.
const FALLBACK_SHELL: &str = "/bin/sh";

/// The holder this build provides, when the caller injects none.
#[cfg(feature = "local-pty")]
pub(crate) fn default_holder() -> Arc<dyn PtyHolder> {
    Arc::new(efr_pty::LocalPtyHolder::new())
}

/// The holder this build provides, when the caller injects none: this build has none.
#[cfg(not(feature = "local-pty"))]
pub(crate) fn default_holder() -> Arc<dyn PtyHolder> {
    Arc::new(no_holder::NoHolder)
}

/// What the shell manager is built from.
#[derive(Debug)]
pub(crate) struct ShellParts {
    pub(crate) settings: ShellSettings,
    pub(crate) integration_dir: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
    /// The programs that the machine policy's command rules name.
    pub(crate) trusted_programs: Vec<String>,
    pub(crate) holder: Arc<dyn PtyHolder>,
    pub(crate) screens: Arc<dyn ScreenFactory>,
    pub(crate) recording: Arc<StoreRecording>,
    pub(crate) notices: Arc<ShellNotices>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) rng: Arc<dyn Rng>,
}

/// The shell manager. When the configured shell (or zsh on the `PATH`) cannot be
/// found, the manager uses `/bin/sh` and logs why, so the daemon still starts.
pub(crate) fn sessions(parts: ShellParts) -> Result<ShellSessions, DaemonError> {
    let ShellParts {
        settings,
        integration_dir,
        env,
        trusted_programs,
        holder,
        screens,
        recording,
        notices,
        clock,
        rng,
    } = parts;
    let mut config = ShellConfig::new(integration_dir, env);
    config.program.clone_from(&settings.program);
    config.login = settings.login;
    config.trusted_programs = trusted_programs;
    let deps = ShellDeps::new(holder, screens, clock, rng)
        .with_recording(recording)
        .with_observer(notices);
    match ShellSessions::new(config.clone(), deps.clone()) {
        Ok(sessions) => Ok(sessions),
        Err(error @ ShellError::ProgramNotFound { .. }) => {
            tracing::error!(error = %error, fallback = FALLBACK_SHELL, "no zsh for the hidden shells; using the fallback without the integration");
            config.program = Some(PathBuf::from(FALLBACK_SHELL));
            ShellSessions::new(config, deps).map_err(DaemonError::from)
        }
        Err(error) => Err(DaemonError::from(error)),
    }
}

/// The directory for the zsh integration files: `$XDG_RUNTIME_DIR/efr/zsh`.
pub(crate) fn integration_dir(runtime: &Path) -> PathBuf {
    runtime.join("zsh")
}

/// The programs that a command rule of `policy` names, each once, in the order of the
/// rules. A rule judges a command by its program's name, so the hidden zsh removes an
/// alias or a function of that name that the user's startup files define.
pub(crate) fn trusted_programs(policy: &Policy) -> Vec<String> {
    let mut programs: Vec<String> = Vec::new();
    for rule in policy.rules() {
        if let Resource::Command(pattern) = &rule.resource
            && !programs.contains(&pattern.program)
        {
            programs.push(pattern.program.clone());
        }
    }
    programs
}

#[cfg(test)]
mod tests;
