//! End-to-end tests over a real zsh on a real PTY (`efr_pty::LocalPtyHolder`), watched
//! through a vt100 screen. They run only when `EFR_TEST_ZSH=1` and say so otherwise;
//! `just test-shell` sets it. Each test gets a throwaway home with empty startup files,
//! so no test reads the user's real home or zsh configuration. The clock is manual:
//! a test that needs a timeout moves it, so nothing waits on real time.
//!
//! The module is named `e2e_zsh` so that every test path starts with `e2e_`, which is
//! what the nextest filter of the serial `shell` group matches.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::ConversationId;
use efr_pty::LocalPtyHolder;
use efr_stdx::env::Var;
use efr_test_support::{TestClock, TestRng};

use crate::testing::{Notices, Vt100Screens};
use crate::{RecordingSink, ShellConfig, ShellDeps, ShellObserver, ShellSessions};

/// A manager over a real zsh in a throwaway home.
pub(crate) struct Zsh {
    pub(crate) sessions: ShellSessions,
    pub(crate) clock: TestClock,
    pub(crate) notices: Arc<Notices>,
    pub(crate) conversation: ConversationId,
    /// The temporary tree: `home/` and `zsh/` (the integration directory).
    pub(crate) root: tempfile::TempDir,
}

impl Zsh {
    /// The harness, or `None` with a message when `EFR_TEST_ZSH` is off.
    pub(crate) fn start(test: &str) -> Option<Self> {
        Self::start_with(test, |_| {})
    }

    /// The harness with `configure` applied to the shell config, or `None` with a
    /// message when `EFR_TEST_ZSH` is off.
    pub(crate) fn start_with(test: &str, configure: impl FnOnce(&mut ShellConfig)) -> Option<Self> {
        if !enabled(test) {
            return None;
        }
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        // Empty startup files: a home without any would make some zsh builds offer
        // their new-user wizard, which waits for input.
        for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            std::fs::write(home.join(file), "").unwrap();
        }
        let base_env = BTreeMap::from([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("SHELL".to_owned(), "/usr/bin/zsh".to_owned()),
        ]);
        let mut config = ShellConfig::new(root.path().join("zsh"), base_env);
        // A login shell would also run the system's /etc/profile, which is not under
        // test here.
        config.login = false;
        configure(&mut config);
        let clock = TestClock::new();
        let notices = Notices::new();
        let deps = ShellDeps::new(
            Arc::new(LocalPtyHolder::new()),
            Arc::new(Vt100Screens),
            clock.shared(),
            Arc::new(TestRng::new(42)),
        )
        .with_recording(Arc::new(crate::testing::Recorded::default()) as Arc<dyn RecordingSink>)
        .with_observer(Arc::clone(&notices) as Arc<dyn ShellObserver>);
        let sessions = ShellSessions::new(config, deps).unwrap();
        let conversation = "01920000-0000-7000-8000-00000000e2e0".parse().unwrap();
        Some(Zsh { sessions, clock, notices, conversation, root })
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    /// A directory under the throwaway tree, created.
    pub(crate) fn dir(&self, name: &str) -> PathBuf {
        let dir = self.root.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // The shell reports the resolved path of its directory.
        std::fs::canonicalize(dir).unwrap()
    }

    pub(crate) fn start_dir(&self) -> &Path {
        self.root.path()
    }
}

#[expect(clippy::print_stderr, reason = "a skipped test says why, as atuin's e2e tests do")]
fn enabled(test: &str) -> bool {
    let on = efr_stdx::env::flag(Var::TestZsh).unwrap_or(false);
    if !on {
        eprintln!("skipping {test}: set EFR_TEST_ZSH=1 to run the tests that drive a real zsh");
    }
    on
}

#[cfg(test)]
mod tests;
