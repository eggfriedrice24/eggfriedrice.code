//! Fakes and fixtures shared by the unit tests of this crate.
//!
//! Every test runs in a [`Sandbox`]: a temporary directory with a `home` inside it,
//! passed to the code under test as the home directory, so no test reads the real
//! `$HOME`.

use std::path::{Path, PathBuf};

use crate::Home;

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
        // Resolved, so a test can compare it with resolved paths.
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
}
