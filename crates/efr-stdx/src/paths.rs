//! The four efr roots and the files that the daemon and its clients agree on.
//!
//! Each root comes from the first of these that gives one:
//!
//! 1. its own variable, such as `EFR_DATA_DIR`;
//! 2. `EFR_HOME`, with the root's name below it, such as `$EFR_HOME/data`;
//! 3. the XDG base directory with `efr` below it, such as `$XDG_DATA_HOME/efr`, the base
//!    from the XDG base directory specification through `etcetera`;
//! 4. for the runtime root only, `/run/user/<uid>/efr` when `XDG_RUNTIME_DIR` is unset
//!    and `/run/user/<uid>` is a directory of the user's own with mode 0700.
//!
//! A root's own variable names the efr directory itself, not a base: with
//! `EFR_DATA_DIR=/tmp/d` the lock file is `/tmp/d/daemon.lock`, not
//! `/tmp/d/efr/daemon.lock`. So a test or `just run` can put all four roots in one
//! throwaway tree, and `EFR_HOME` does the same with one variable. The zsh plugin finds
//! the runtime root by the same rule.

use std::fmt;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use etcetera::BaseStrategy as _;

use crate::StdxError;
use crate::env::{Env, Var};

/// The directory under each XDG base.
const APP_DIR: &str = "efr";
const SOCKET_FILE: &str = "daemon.sock";
const DAEMON_JSON_FILE: &str = "daemon.json";
const LOCK_FILE: &str = "daemon.lock";

/// The directories below `EFR_HOME`.
const HOME_CONFIG: &str = "config";
const HOME_DATA: &str = "data";
const HOME_STATE: &str = "state";
const HOME_RUNTIME: &str = "runtime";

/// The parent of each user's runtime directory under systemd-logind.
const RUN_USER: &str = "/run/user";

/// The longest path a Unix socket address holds on Linux: `sun_path` is 108 bytes and
/// ends with a NUL.
pub const MAX_SOCKET_PATH: usize = 107;

/// The four efr roots. Every path is absolute.
///
/// ```
/// use std::path::Path;
///
/// use efr_stdx::paths::Dirs;
///
/// let dirs = Dirs::new("/c/efr", "/d/efr", "/s/efr", "/run/user/1000/efr")?;
/// assert_eq!(dirs.socket_path(), Path::new("/run/user/1000/efr/daemon.sock"));
/// assert_eq!(dirs.lock_path(), Path::new("/d/efr/daemon.lock"));
/// # Ok::<(), efr_stdx::StdxError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    runtime: PathBuf,
}

/// Where a root's path came from, in the order [`Dirs::resolve`] looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RootSource {
    /// The root's own variable, such as `EFR_DATA_DIR`.
    Variable(Var),
    /// A directory below `EFR_HOME`, such as `$EFR_HOME/data`.
    EfrHome,
    /// The XDG base directory, such as `$XDG_DATA_HOME/efr`.
    Xdg,
    /// `/run/user/<uid>/efr`, for the runtime root when `XDG_RUNTIME_DIR` is unset.
    RunUser,
}

impl fmt::Display for RootSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RootSource::Variable(var) => write!(f, "{var}"),
            RootSource::EfrHome => f.write_str("EFR_HOME"),
            RootSource::Xdg => f.write_str("XDG"),
            RootSource::RunUser => f.write_str(RUN_USER),
        }
    }
}

/// Where each of the four roots came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RootSources {
    /// The config root's source.
    pub config: RootSource,
    /// The data root's source.
    pub data: RootSource,
    /// The state root's source.
    pub state: RootSource,
    /// The runtime root's source.
    pub runtime: RootSource,
}

/// The XDG base directories, before `efr` is appended.
#[derive(Debug, Clone)]
pub(crate) struct XdgBases {
    pub(crate) config: PathBuf,
    pub(crate) data: PathBuf,
    pub(crate) state: PathBuf,
    /// `None` when `XDG_RUNTIME_DIR` is unset; the specification has no default.
    pub(crate) runtime: Option<PathBuf>,
    /// `/run/user/<uid>` when it is a directory of the user's own with mode 0700, the
    /// fallback for an unset `XDG_RUNTIME_DIR`.
    pub(crate) run_user: Option<PathBuf>,
}

impl Dirs {
    /// Resolves the roots from the process environment.
    ///
    /// Fails when a root has neither its own variable nor `EFR_HOME` and its XDG base
    /// cannot be found: the home directory is unknown, or, for the runtime root,
    /// `XDG_RUNTIME_DIR` is unset and `/run/user/<uid>` is not usable. A variable that
    /// does not hold an absolute path is an error too.
    pub fn resolve() -> Result<Self, StdxError> {
        Self::resolve_with_sources().map(|(dirs, _)| dirs)
    }

    /// Like [`resolve`](Self::resolve), with where each root came from.
    pub fn resolve_with_sources() -> Result<(Self, RootSources), StdxError> {
        Self::resolve_in(&Env::process(), xdg_bases)
    }

    /// Roots at explicit paths, for tests and tools that manage their own tree.
    ///
    /// Every path must be absolute.
    pub fn new(
        config: impl Into<PathBuf>,
        data: impl Into<PathBuf>,
        state: impl Into<PathBuf>,
        runtime: impl Into<PathBuf>,
    ) -> Result<Self, StdxError> {
        Ok(Dirs {
            config: absolute(config.into())?,
            data: absolute(data.into())?,
            state: absolute(state.into())?,
            runtime: absolute(runtime.into())?,
        })
    }

    pub(crate) fn resolve_in<F>(env: &Env, bases: F) -> Result<(Self, RootSources), StdxError>
    where
        F: FnOnce() -> Result<XdgBases, StdxError>,
    {
        let home = env.path(Var::Home)?;
        let pick = |var: Var, below: &str| -> Result<Option<(PathBuf, RootSource)>, StdxError> {
            Ok(match env.path(var)? {
                Some(path) => Some((path, RootSource::Variable(var))),
                None => home.as_ref().map(|home| (home.join(below), RootSource::EfrHome)),
            })
        };
        let chosen = (
            pick(Var::ConfigDir, HOME_CONFIG)?,
            pick(Var::DataDir, HOME_DATA)?,
            pick(Var::StateDir, HOME_STATE)?,
            pick(Var::RuntimeDir, HOME_RUNTIME)?,
        );
        let (config, data, state, runtime) = match chosen {
            // NOTE: the XDG lookup needs a home directory. Skipping it when every root
            // is chosen, by its own variable or by EFR_HOME, lets efr run in a sandbox
            // that has no HOME.
            (Some(config), Some(data), Some(state), Some(runtime)) => {
                (config, data, state, runtime)
            }
            (config, data, state, runtime) => {
                let bases = bases()?;
                let xdg = |base: PathBuf| (base.join(APP_DIR), RootSource::Xdg);
                let runtime = match (runtime, bases.runtime, bases.run_user) {
                    (Some(runtime), _, _) => runtime,
                    (None, Some(base), _) => xdg(base),
                    (None, None, Some(run_user)) => (run_user.join(APP_DIR), RootSource::RunUser),
                    (None, None, None) => return Err(StdxError::RuntimeDirUnset),
                };
                (
                    config.unwrap_or_else(|| xdg(bases.config)),
                    data.unwrap_or_else(|| xdg(bases.data)),
                    state.unwrap_or_else(|| xdg(bases.state)),
                    runtime,
                )
            }
        };
        let sources =
            RootSources { config: config.1, data: data.1, state: state.1, runtime: runtime.1 };
        let dirs = Dirs { config: config.0, data: data.0, state: state.0, runtime: runtime.0 };
        Ok((dirs, sources))
    }

    /// Configuration: `config.toml` and the project registry.
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// Persistent data: the database, recordings, scratch directories and secrets.
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// State that may be lost without harm, such as logs.
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Files that live only while the user is logged in: the socket and `daemon.json`.
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    /// The daemon's Unix socket.
    pub fn socket_path(&self) -> PathBuf {
        self.runtime.join(SOCKET_FILE)
    }

    /// The daemon's Unix socket, or an error when its path is longer than
    /// [`MAX_SOCKET_PATH`] bytes, so no socket can be bound or reached there. A runtime
    /// root below `EFR_HOME` lives on disk, often deep in a home directory, so the
    /// daemon checks this before it binds.
    pub fn checked_socket_path(&self) -> Result<PathBuf, StdxError> {
        let path = self.socket_path();
        if path.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(StdxError::SocketPathTooLong { path });
        }
        Ok(path)
    }

    /// The discovery file that the daemon writes next to its socket.
    pub fn daemon_json_path(&self) -> PathBuf {
        self.runtime.join(DAEMON_JSON_FILE)
    }

    /// The file whose exclusive lock decides which daemon is the single instance. It
    /// lives in the data root, not the runtime root, because it guards the database.
    pub fn lock_path(&self) -> PathBuf {
        self.data.join(LOCK_FILE)
    }
}

fn absolute(path: PathBuf) -> Result<PathBuf, StdxError> {
    if path.is_absolute() { Ok(path) } else { Err(StdxError::NotAbsolute { path }) }
}

fn xdg_bases() -> Result<XdgBases, StdxError> {
    let xdg = etcetera::base_strategy::Xdg::new().map_err(|_| StdxError::HomeNotFound)?;
    // etcetera's Xdg strategy always returns a state directory; the fallback is the
    // specification's default in case a later version stops doing so.
    let state = xdg.state_dir().unwrap_or_else(|| xdg.home_dir().join(".local/state"));
    let runtime = xdg.runtime_dir();
    // NOTE: /run/user is looked at only when XDG_RUNTIME_DIR is unset, as in a cron job
    // or an ssh session without pam_systemd.
    let run_user = match runtime {
        Some(_) => None,
        None => {
            let uid = rustix::process::getuid().as_raw();
            let dir = Path::new(RUN_USER).join(uid.to_string());
            usable_run_user(&dir, uid).then_some(dir)
        }
    };
    Ok(XdgBases { config: xdg.config_dir(), data: xdg.data_dir(), state, runtime, run_user })
}

/// True when `dir` is a directory, not a link, owned by `uid` with mode 0700, as
/// systemd-logind creates `/run/user/<uid>`. Anything else may belong to someone else,
/// so the socket must not go there.
pub(crate) fn usable_run_user(dir: &Path, uid: u32) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|meta| {
        meta.is_dir() && meta.uid() == uid && meta.permissions().mode() & 0o7777 == 0o700
    })
}

#[cfg(test)]
mod tests;
