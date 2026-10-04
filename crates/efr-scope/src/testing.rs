//! Fakes and fixtures shared by the unit tests of this crate.
//!
//! Every test runs in a [`Sandbox`]: a temporary directory with a `home` inside it,
//! passed to the code under test as the home directory, so no test reads the real
//! `$HOME` or the user's git configuration.

use std::future::{pending, ready};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::{Clock, Sleep};

use crate::{Git, Home};

/// A clock whose sleeps never finish, so a git timeout never fires and no test waits
/// on real time.
#[derive(Debug)]
pub(crate) struct StoppedClock;

impl Clock for StoppedClock {
    fn now(&self) -> jiff::Timestamp {
        jiff::Timestamp::UNIX_EPOCH
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(pending())
    }
}

/// A clock whose sleeps finish at once, so every git timeout fires at its first poll.
#[derive(Debug)]
pub(crate) struct InstantClock;

impl Clock for InstantClock {
    fn now(&self) -> jiff::Timestamp {
        jiff::Timestamp::UNIX_EPOCH
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(ready(()))
    }
}

/// The git runner of the tests: isolated from the user's configuration, never timing
/// out.
pub(crate) fn git() -> Git {
    Git::new(Arc::new(StoppedClock)).isolated()
}

/// A temporary directory with a home directory inside it.
#[derive(Debug)]
pub(crate) struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: Home,
}

impl Sandbox {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Resolved, because git reports resolved paths.
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("home")).unwrap();
        let home = Home::new(root.join("home")).unwrap();
        Sandbox { _dir: dir, root, home }
    }

    /// The sandbox itself, the parent of the home directory.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn home(&self) -> &Home {
        &self.home
    }

    /// A path in the home directory.
    pub(crate) fn in_home(&self, relative: &str) -> PathBuf {
        self.home.path().join(relative)
    }

    /// Creates a directory and its parents, and returns it.
    pub(crate) fn mkdir(&self, path: &Path) -> PathBuf {
        std::fs::create_dir_all(path).unwrap();
        path.to_path_buf()
    }

    /// Runs real git in `cwd` with the sandbox as its whole world and asserts that it
    /// succeeds.
    pub(crate) async fn git(&self, cwd: &Path, args: &[&str]) {
        let status = efr_stdx::process::command("git", cwd)
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "user.name=efr",
                "-c",
                "user.email=efr@test",
            ])
            .args(args)
            .env("HOME", self.home.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git {args:?} in {} failed", cwd.display());
    }

    /// `git init` at `dir`, creating it first.
    pub(crate) async fn init(&self, dir: &Path) -> PathBuf {
        self.mkdir(dir);
        self.git(dir, &["init", "--quiet"]).await;
        dir.to_path_buf()
    }
}
