//! What the daemon tells the shell manager, and what it hands it.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_holder::{PtyHolder, Size};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;

use crate::{Discard, RecordingSink, ScreenFactory, ShellObserver};

/// How hidden shells are started. The daemon builds it from its own config and passes
/// it by value; nothing in this crate reads the environment.
///
/// `Debug` lists the names in [`base_env`](Self::base_env) but not their values,
/// because the user's environment can hold tokens.
#[derive(Clone)]
#[non_exhaustive]
pub struct ShellConfig {
    /// The absolute path of the shell. `None` looks for `zsh` on the `PATH` of
    /// [`base_env`](Self::base_env). A shell whose file name does not start with `zsh`
    /// gets no integration, so every run is delimited with a sentinel.
    pub program: Option<PathBuf>,
    /// Starts the shell as a login shell (`-l`), so `/etc/zprofile`, `.zprofile` and
    /// `.zlogin` run as in a terminal's first shell. The daemon runs as a service whose
    /// environment came from systemd, not from a login.
    pub login: bool,
    /// Where the ZDOTDIR shim and the integration script are written. The daemon
    /// passes a directory of its own, such as `$XDG_RUNTIME_DIR/efr/zsh`.
    pub integration_dir: PathBuf,
    /// The user's environment, which every hidden shell inherits after the variables
    /// that belong to the daemon or to another terminal are removed (see the crate
    /// README).
    pub base_env: BTreeMap<String, String>,
    /// The terminal size of a new shell.
    pub size: Size,
    /// `TERM` in the shell.
    pub term: String,
    /// `COLORTERM` in the shell.
    pub colorterm: String,
    /// How long a new zsh may take to show its first marked prompt before its runs
    /// fall back to sentinels.
    pub startup_timeout: Duration,
    /// At a run's timeout, a command that printed nothing for this long and left the
    /// cursor after some text counts as waiting for input.
    pub quiet_period: Duration,
    /// How long [`close`](crate::ShellSessions::close) waits for the shell to end
    /// after `SIGHUP` before it sends `SIGKILL`.
    pub close_grace: Duration,
}

impl ShellConfig {
    /// The size of a new shell unless the config says otherwise.
    pub const DEFAULT_SIZE: Size = Size { cols: 160, rows: 48 };

    /// A config with the defaults: zsh from the `PATH`, an interactive login shell,
    /// [`DEFAULT_SIZE`](Self::DEFAULT_SIZE), `xterm-256color` with truecolor, ten
    /// seconds to start, one second of quiet for an input prompt and five seconds to
    /// close.
    pub fn new(integration_dir: impl Into<PathBuf>, base_env: BTreeMap<String, String>) -> Self {
        ShellConfig {
            program: None,
            login: true,
            integration_dir: integration_dir.into(),
            base_env,
            size: Self::DEFAULT_SIZE,
            term: "xterm-256color".to_owned(),
            colorterm: "truecolor".to_owned(),
            startup_timeout: Duration::from_secs(10),
            quiet_period: Duration::from_secs(1),
            close_grace: Duration::from_secs(5),
        }
    }
}

impl fmt::Debug for ShellConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShellConfig")
            .field("program", &self.program)
            .field("login", &self.login)
            .field("integration_dir", &self.integration_dir)
            .field("base_env", &self.base_env.keys().collect::<Vec<_>>())
            .field("size", &self.size)
            .field("term", &self.term)
            .field("colorterm", &self.colorterm)
            .field("startup_timeout", &self.startup_timeout)
            .field("quiet_period", &self.quiet_period)
            .field("close_grace", &self.close_grace)
            .finish()
    }
}

/// The collaborators of the shell manager, injected so the daemon chooses them and a
/// test replaces them.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ShellDeps {
    /// Opens the PTYs and keeps the shell processes.
    pub holder: Arc<dyn PtyHolder>,
    /// Builds one screen per shell; the daemon chooses vt100 or ghostty here.
    pub screens: Arc<dyn ScreenFactory>,
    /// Receives every byte read from every shell.
    pub recording: Arc<dyn RecordingSink>,
    /// Hears when a shell starts, changes directory and exits.
    pub observer: Arc<dyn ShellObserver>,
    /// Times startups, runs and closes.
    pub clock: Arc<dyn Clock>,
    /// Mints PTY ids and sentinel tokens.
    pub rng: Arc<dyn Rng>,
}

impl ShellDeps {
    /// Collaborators that discard the recording and every notice until
    /// [`with_recording`](Self::with_recording) and
    /// [`with_observer`](Self::with_observer) say otherwise.
    pub fn new(
        holder: Arc<dyn PtyHolder>,
        screens: Arc<dyn ScreenFactory>,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Self {
        ShellDeps {
            holder,
            screens,
            recording: Arc::new(Discard),
            observer: Arc::new(Discard),
            clock,
            rng,
        }
    }

    /// Sends the PTY bytes to `recording`.
    #[must_use]
    pub fn with_recording(mut self, recording: Arc<dyn RecordingSink>) -> Self {
        self.recording = recording;
        self
    }

    /// Sends the lifecycle notices to `observer`.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn ShellObserver>) -> Self {
        self.observer = observer;
        self
    }
}

#[cfg(test)]
mod tests;
