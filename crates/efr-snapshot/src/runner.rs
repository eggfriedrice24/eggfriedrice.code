//! How this crate runs git on a store: hardened, under a timeout, off the async
//! workers.
//!
//! Every command goes through `efr_scope::Git::command` (no prompt, no optional locks,
//! the variables that point git elsewhere removed) and then gets the store's own
//! `GIT_DIR` and `GIT_INDEX_FILE`, the root as `GIT_WORK_TREE` when it reads files, no
//! system or global config, and `-c` settings that switch off everything that could
//! run code or write outside the store: fsmonitor, hooks, the untracked cache,
//! automatic gc, line-end conversion. No filter or diff driver is defined, so a
//! `.gitattributes` in a root cannot run anything. The project's own `.git`, its config
//! and its index are never read or written.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_scope::{Git, Home};
use efr_stdx::time::Clock;
use tokio::io::AsyncWriteExt as _;

use crate::SnapshotError;
use crate::store::Store;

/// The `-c` settings of every command.
const HARDENING: &[&str] = &[
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    "core.untrackedCache=false",
    "core.autocrlf=false",
    "core.safecrlf=false",
    "core.symlinks=true",
    "core.quotePath=false",
    "gc.auto=0",
    "gc.autoDetach=false",
    "maintenance.auto=false",
    "diff.noprefix=false",
    "diff.renames=true",
    "commit.gpgSign=false",
];

/// Variables that would change what a command does, beyond those `Git::command`
/// removes.
const SCRUBBED_ENV: &[&str] = &[
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_ATTR_SOURCE",
    "GIT_REPLACE_REF_BASE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_SHALLOW_FILE",
    "GIT_QUARANTINE_PATH",
    "GIT_ICASE_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_TRACE",
    "GIT_EDITOR",
    "GIT_PAGER",
];

/// What one command printed and whether it succeeded.
#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
}

/// One command on a store.
#[derive(Debug, Default)]
pub(crate) struct Run<'a> {
    /// The root, set as `GIT_WORK_TREE` and the working directory; without it the
    /// command runs in the store's directory and reads no file of the root.
    pub(crate) work_tree: Option<&'a Path>,
    /// Bytes for the command's standard input.
    pub(crate) stdin: Option<Vec<u8>>,
    /// Reads every pathspec literally, so a file name such as `*` or `:(top)` names
    /// only itself.
    pub(crate) literal: bool,
    /// The author and committer time of a commit, in git's `<seconds> +0000` form.
    pub(crate) commit_time: Option<String>,
}

/// Runs git on stores.
#[derive(Debug, Clone)]
pub(crate) struct Runner {
    git: Git,
    home: Home,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    excludes_file: Option<PathBuf>,
}

impl Runner {
    pub(crate) fn new(
        git: Git,
        home: Home,
        clock: Arc<dyn Clock>,
        timeout: Duration,
        excludes_file: Option<PathBuf>,
    ) -> Self {
        Runner { git, home, clock, timeout, excludes_file }
    }

    pub(crate) fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// Runs `git <args>` on `store` as `run` says.
    pub(crate) async fn run<S: AsRef<OsStr>>(
        &self,
        store: &Store,
        args: &[S],
        run: Run<'_>,
    ) -> Result<Output, SnapshotError> {
        let cwd = run.work_tree.unwrap_or(store.dir());
        let mut all: Vec<OsString> = Vec::new();
        for setting in HARDENING {
            all.push("-c".into());
            all.push((*setting).into());
        }
        if let Some(file) = &self.excludes_file {
            let mut setting = OsString::from("core.excludesFile=");
            setting.push(file);
            all.push("-c".into());
            all.push(setting);
        }
        all.extend(args.iter().map(|arg| arg.as_ref().to_owned()));
        let mut command = self.git.command(cwd, &self.home, &all);
        for name in SCRUBBED_ENV {
            command.env_remove(name);
        }
        command
            .env("GIT_DIR", store.git_dir())
            .env("GIT_INDEX_FILE", store.index())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        match run.work_tree {
            Some(root) => command.env("GIT_WORK_TREE", root),
            None => command.env_remove("GIT_WORK_TREE"),
        };
        if run.literal {
            command.env("GIT_LITERAL_PATHSPECS", "1");
        } else {
            command.env_remove("GIT_LITERAL_PATHSPECS");
        }
        if let Some(time) = &run.commit_time {
            command
                .env("GIT_AUTHOR_NAME", "efr")
                .env("GIT_AUTHOR_EMAIL", "efr@localhost")
                .env("GIT_AUTHOR_DATE", time)
                .env("GIT_COMMITTER_NAME", "efr")
                .env("GIT_COMMITTER_EMAIL", "efr@localhost")
                .env("GIT_COMMITTER_DATE", time);
        }
        if run.stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        let stdin = run.stdin;
        let work = async move {
            // NOTE: spawning returns only once the child has changed into its working
            // directory, and on a hung mount that never ends; the blocking pool holds
            // it instead of an async worker.
            let mut child = tokio::task::spawn_blocking(move || command.spawn())
                .await
                .map_err(io::Error::other)??;
            if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
                pipe.write_all(&bytes).await?;
                drop(pipe);
            }
            child.wait_with_output().await
        };
        match self.clock.timeout(self.timeout, work).await {
            Ok(Ok(output)) => {
                Ok(Output { success: output.status.success(), stdout: output.stdout })
            }
            Ok(Err(source)) => Err(SnapshotError::RunGit { program: "git".into(), source }),
            Err(_) => Err(SnapshotError::GitTimedOut { after: self.timeout }),
        }
    }

    /// Runs `git <args>` and fails when git fails; `command` names it in the error.
    pub(crate) async fn checked<S: AsRef<OsStr>>(
        &self,
        store: &Store,
        command: &'static str,
        args: &[S],
        run: Run<'_>,
    ) -> Result<Vec<u8>, SnapshotError> {
        let output = self.run(store, args, run).await?;
        if output.success {
            Ok(output.stdout)
        } else {
            Err(SnapshotError::GitFailed { command, store: store.git_dir().to_path_buf() })
        }
    }

    /// Creates the bare repository of `store`: `git init --bare` with no template, so
    /// it holds no sample hooks, outside any root.
    pub(crate) async fn init(&self, store: &Store) -> Result<(), SnapshotError> {
        let mut args: Vec<OsString> = vec![
            "-c".into(),
            "init.defaultBranch=efr".into(),
            "init".into(),
            "--bare".into(),
            "--quiet".into(),
            "--template=".into(),
        ];
        args.push(store.git_dir().as_os_str().to_owned());
        let mut command = self.git.command(store.dir(), &self.home, &args);
        command
            .env_remove("GIT_DIR")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        let work = async move {
            let child = tokio::task::spawn_blocking(move || command.spawn())
                .await
                .map_err(io::Error::other)??;
            child.wait_with_output().await
        };
        match self.clock.timeout(self.timeout, work).await {
            Ok(Ok(output)) if output.status.success() => Ok(()),
            Ok(Ok(_)) => Err(SnapshotError::GitFailed {
                command: "init",
                store: store.git_dir().to_path_buf(),
            }),
            Ok(Err(source)) => Err(SnapshotError::RunGit { program: "git".into(), source }),
            Err(_) => Err(SnapshotError::GitTimedOut { after: self.timeout }),
        }
    }

    /// Runs `job`, a look at the file system, on the blocking pool under the timeout.
    pub(crate) async fn blocking<T, F>(&self, path: &Path, job: F) -> Result<T, SnapshotError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        match self.clock.timeout(self.timeout, tokio::task::spawn_blocking(job)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(SnapshotError::TaskFailed),
            Err(_) => Err(SnapshotError::InspectTimedOut {
                path: path.to_path_buf(),
                after: self.timeout,
            }),
        }
    }
}
