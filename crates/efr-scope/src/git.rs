//! Guarded git discovery.
//!
//! Plain git discovery walks up from a directory to the first `.git` it finds. In a
//! dotfiles home that is `~/.git`, which would make every directory under `~` look
//! like one repository rooted at `~`. Two guards stop that:
//!
//! - git runs with `GIT_CEILING_DIRECTORIES` set to `$HOME` and `/`, so it never climbs
//!   into either while it searches;
//! - a work tree whose root is `$HOME`, `/` or a directory above `$HOME` (git finds one
//!   when it starts in that directory) is reported as [`Discovery::Guarded`], never as a
//!   work tree.
//!
//! git also runs without the variables that point it at another repository or inject
//! configuration, with optional locks and terminal prompts off, and under a timeout on
//! the injected clock, because the next turn waits for it.
//!
//! Every look at the file system that discovery and derivation make (`is_dir`,
//! `canonicalize`, listing `$HOME`) and the start of git itself run on tokio's blocking
//! pool under the same timeout, through `Git::probe`: on a hung network mount a
//! `stat` never returns, and it must hold neither an async worker nor the turn.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock;

use crate::home::normalize;
use crate::{Home, ScopeError};

/// How long one git command may take before discovery gives up.
pub const DEFAULT_GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Variables that would make git look at another repository than the one around the
/// working directory, or read configuration from the environment.
const SCRUBBED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

/// What git found around a directory.
///
/// The enum is deliberately exhaustive: whoever reads it must decide what a guarded
/// work tree means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    /// No work tree holds the directory, the directory does not exist, or git refused
    /// the repository (one owned by another user, for example).
    NotARepository,
    /// The work tree's root is `$HOME`, `/` or a directory above `$HOME`. It is never a
    /// project root on its own.
    Guarded {
        /// The root that git reported.
        root: PathBuf,
    },
    /// A work tree below `$HOME` or outside it.
    WorkTree(Repo),
}

impl Discovery {
    /// The work tree, when discovery found one that is not guarded.
    pub fn work_tree(&self) -> Option<&Repo> {
        match self {
            Discovery::WorkTree(repo) => Some(repo),
            Discovery::NotARepository | Discovery::Guarded { .. } => None,
        }
    }
}

/// A git work tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The top of the work tree, as git reports it (symbolic links resolved).
    pub root: PathBuf,
    /// The checked-out branch, or `None` when `HEAD` is detached or the name is not
    /// valid UTF-8.
    pub branch: Option<String>,
}

/// How this crate runs git.
#[derive(Debug, Clone)]
pub struct Git {
    program: OsString,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    isolated: bool,
}

impl Git {
    /// Runs `git` from `PATH`, with the user's git configuration and
    /// [`DEFAULT_GIT_TIMEOUT`] measured on `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Git { program: OsString::from("git"), clock, timeout: DEFAULT_GIT_TIMEOUT, isolated: false }
    }

    /// Ignores the system and global git configuration (`/etc/gitconfig`,
    /// `~/.gitconfig`, `$XDG_CONFIG_HOME/git/config`), for tests and test daemons.
    pub fn isolated(mut self) -> Self {
        self.isolated = true;
        self
    }

    /// Runs `program` instead of `git`.
    pub fn with_program(mut self, program: impl Into<OsString>) -> Self {
        self.program = program.into();
        self
    }

    /// Gives up on a git command after `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Which git work tree holds `cwd`, with the guards of this module.
    ///
    /// Fails when `cwd` is relative, when git cannot be started, or when git or the look
    /// at `cwd` does not finish in time.
    pub async fn discover(&self, cwd: &Path, home: &Home) -> Result<Discovery, ScopeError> {
        let Some(cwd) = normalize(cwd) else {
            return Err(ScopeError::NotAbsolute { path: cwd.to_path_buf() });
        };
        let target = cwd.clone();
        if !self.probe(&cwd, move || target.is_dir()).await? {
            return Ok(Discovery::NotARepository);
        }
        let Some(root) = self.run(&cwd, home, ["rev-parse", "--show-toplevel"]).await? else {
            return Ok(Discovery::NotARepository);
        };
        let root = PathBuf::from(OsString::from_vec(root));
        if !root.is_absolute() {
            return Ok(Discovery::NotARepository);
        }
        if home.is_at_or_above(&root) {
            return Ok(Discovery::Guarded { root });
        }
        let branch = self
            .run(&cwd, home, ["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await?
            .and_then(|name| String::from_utf8(name).ok());
        Ok(Discovery::WorkTree(Repo { root, branch }))
    }

    /// Runs git in `cwd` and returns its standard output without the final newline, or
    /// `None` when git exits with a failure status. The command is [`Git::command`]'s,
    /// and it gives up after this runner's timeout.
    pub async fn run<I, S>(
        &self,
        cwd: &Path,
        home: &Home,
        args: I,
    ) -> Result<Option<Vec<u8>>, ScopeError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(cwd, home, args);
        let output = async move {
            // NOTE: spawning returns only once the child has changed into `cwd` and
            // executed git, and on a hung mount that change never ends.
            let child = tokio::task::spawn_blocking(move || command.spawn())
                .await
                .map_err(io::Error::other)??;
            child.wait_with_output().await
        };
        let output = match self.clock.timeout(self.timeout, output).await {
            Ok(Ok(output)) => output,
            Ok(Err(source)) => {
                return Err(ScopeError::RunGit { program: self.program.clone(), source });
            }
            Err(_) => return Err(ScopeError::GitTimedOut { after: self.timeout }),
        };
        if !output.status.success() {
            return Ok(None);
        }
        let mut stdout = output.stdout;
        if stdout.last() == Some(&b'\n') {
            stdout.pop();
        }
        Ok(Some(stdout))
    }

    /// Runs `probe`, a look at the file system around `path`, on tokio's blocking pool
    /// under this runner's timeout.
    ///
    /// A `stat` or `realpath` on a hung network mount never returns. On the blocking
    /// pool it holds one pool thread instead of an async worker, and the timeout lets the
    /// turn go on with `Machine`.
    pub(crate) async fn probe<T, F>(&self, path: &Path, probe: F) -> Result<T, ScopeError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        match self.clock.timeout(self.timeout, tokio::task::spawn_blocking(probe)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(source)) => Err(ScopeError::InspectAborted { path: path.to_path_buf(), source }),
            Err(_) => {
                Err(ScopeError::InspectTimedOut { path: path.to_path_buf(), after: self.timeout })
            }
        }
    }

    /// The command for one git run, with the environment of this module: no prompt, no
    /// optional locks, the ceilings at the home directory, the variables that point git
    /// elsewhere removed, and without the user's configuration when isolated. A caller
    /// that runs git on a repository the model can write adds `-c core.fsmonitor=false
    /// -c core.hooksPath=/dev/null` itself.
    pub fn command<I, S>(&self, cwd: &Path, home: &Home, args: I) -> tokio::process::Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = efr_stdx::process::command(&self.program, cwd);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // A timed-out git must not outlive the turn that gave up on it.
            .kill_on_drop(true);
        for name in SCRUBBED_ENV {
            command.env_remove(name);
        }
        command
            .env("HOME", home.path())
            .env("GIT_CEILING_DIRECTORIES", ceilings(home))
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0");
        if self.isolated {
            command
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("XDG_CONFIG_HOME", home.path().join(".config"));
        }
        command
    }
}

/// `$HOME` in both forms, then `/`, joined with `:`. A form that holds `:` itself
/// cannot be listed and is left out; the guard on the reported root still holds.
fn ceilings(home: &Home) -> OsString {
    let mut list = OsString::new();
    let mut forms = vec![home.path()];
    if home.canonical() != home.path() {
        forms.push(home.canonical());
    }
    for form in forms {
        if !form.as_os_str().as_encoded_bytes().contains(&b':') {
            list.push(form);
            list.push(":");
        }
    }
    list.push("/");
    list
}

#[cfg(test)]
mod tests;
