//! The four efr roots and the files that the daemon and its clients agree on.
//!
//! Each root is `<XDG base>/efr`, with the base from the XDG base directory
//! specification through `etcetera`, unless its `EFR_*` variable names a replacement.
//! A replacement is the efr directory itself, not a base: with `EFR_DATA_DIR=/tmp/d`
//! the lock file is `/tmp/d/daemon.lock`, not `/tmp/d/efr/daemon.lock`. So a test or
//! `just run` can put all four roots in one throwaway tree.

use std::path::{Path, PathBuf};

use etcetera::BaseStrategy as _;

use crate::StdxError;
use crate::env::{Env, Var};

/// The directory under each XDG base.
const APP_DIR: &str = "efr";
const SOCKET_FILE: &str = "daemon.sock";
const DAEMON_JSON_FILE: &str = "daemon.json";
const LOCK_FILE: &str = "daemon.lock";

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

/// The XDG base directories, before `efr` is appended.
#[derive(Debug, Clone)]
pub(crate) struct XdgBases {
    pub(crate) config: PathBuf,
    pub(crate) data: PathBuf,
    pub(crate) state: PathBuf,
    /// `None` when `XDG_RUNTIME_DIR` is unset; the specification has no default.
    pub(crate) runtime: Option<PathBuf>,
}

impl Dirs {
    /// Resolves the roots from the process environment.
    ///
    /// Fails when a root has no `EFR_*` replacement and its XDG base cannot be found:
    /// the home directory is unknown, or `XDG_RUNTIME_DIR` is unset for the runtime
    /// root. A replacement that is not an absolute path is an error too.
    pub fn resolve() -> Result<Self, StdxError> {
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

    pub(crate) fn resolve_in<F>(env: &Env, bases: F) -> Result<Self, StdxError>
    where
        F: FnOnce() -> Result<XdgBases, StdxError>,
    {
        let replaced = (
            env.path(Var::ConfigDir)?,
            env.path(Var::DataDir)?,
            env.path(Var::StateDir)?,
            env.path(Var::RuntimeDir)?,
        );
        match replaced {
            // NOTE: the XDG lookup needs a home directory. Skipping it when every root
            // is replaced lets efr run in a sandbox that has no HOME.
            (Some(config), Some(data), Some(state), Some(runtime)) => {
                Ok(Dirs { config, data, state, runtime })
            }
            (config, data, state, runtime) => {
                let bases = bases()?;
                let runtime = match runtime {
                    Some(runtime) => runtime,
                    None => bases.runtime.ok_or(StdxError::RuntimeDirUnset)?.join(APP_DIR),
                };
                Ok(Dirs {
                    config: config.unwrap_or_else(|| bases.config.join(APP_DIR)),
                    data: data.unwrap_or_else(|| bases.data.join(APP_DIR)),
                    state: state.unwrap_or_else(|| bases.state.join(APP_DIR)),
                    runtime,
                })
            }
        }
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
    Ok(XdgBases {
        config: xdg.config_dir(),
        data: xdg.data_dir(),
        state,
        runtime: xdg.runtime_dir(),
    })
}

#[cfg(test)]
mod tests;
