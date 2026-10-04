//! Temporary efr roots for one test.

use std::ffi::OsString;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use efr_stdx::env::{Env, Var};
use efr_stdx::paths::Dirs;
use tempfile::TempDir;

use crate::TestSupportError;

/// The mode of every directory in the tree, the mode the daemon gives its own.
const DIR_MODE: u32 = 0o700;

/// A throwaway tree that holds the four efr roots and a home directory, removed when
/// the value is dropped.
///
/// The layout under [`root`](TestDirs::root) is `config/`, `data/`, `state/` and
/// `runtime/` (the efr directories themselves, as `EFR_CONFIG_DIR` and the others name
/// them, the same layout `just run` uses) and `home/`, a stand-in for `$HOME` and a
/// place for a test's working directories. All five exist, with mode 0700.
///
/// The root is the real path of the temporary directory, with symbolic links
/// resolved, so a path that the code under test canonicalizes stays equal to the path
/// the test holds.
#[derive(Debug)]
pub struct TestDirs {
    // NOTE: held for its Drop, which removes the tree.
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
    dirs: Dirs,
}

impl TestDirs {
    /// Creates the tree in the system's temporary directory.
    pub fn new() -> Result<Self, TestSupportError> {
        let temp = tempfile::Builder::new()
            .prefix("efr-test-")
            .tempdir()
            .map_err(|source| TestSupportError::CreateTempDir { source })?;
        let root = temp.path().canonicalize().map_err(|source| {
            TestSupportError::ResolveTempDir { path: temp.path().to_path_buf(), source }
        })?;
        let home = root.join("home");
        let dirs = Dirs::new(
            root.join("config"),
            root.join("data"),
            root.join("state"),
            root.join("runtime"),
        )
        .map_err(|source| TestSupportError::Dirs { source })?;
        for dir in [&home, dirs.config(), dirs.data(), dirs.state(), dirs.runtime()] {
            create_dir(dir)?;
        }
        Ok(TestDirs { _temp: temp, root, home, dirs })
    }

    /// The top of the tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The stand-in home directory, `<root>/home`.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The four efr roots.
    pub fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    /// An environment that names the four roots through `EFR_CONFIG_DIR`,
    /// `EFR_DATA_DIR`, `EFR_STATE_DIR` and `EFR_RUNTIME_DIR`, for code that resolves its
    /// directories from an [`Env`].
    pub fn env(&self) -> Env {
        let dirs = &self.dirs;
        let os = |path: &Path| OsString::from(path.as_os_str());
        Env::fixed([
            (Var::ConfigDir, os(dirs.config())),
            (Var::DataDir, os(dirs.data())),
            (Var::StateDir, os(dirs.state())),
            (Var::RuntimeDir, os(dirs.runtime())),
        ])
    }

    /// Creates `relative` and its parents under the root, mode 0700, and returns its
    /// path. It is not an error when the directory exists.
    pub fn create_dir(&self, relative: impl AsRef<Path>) -> Result<PathBuf, TestSupportError> {
        let path = self.root.join(relative);
        create_dir(&path)?;
        Ok(path)
    }
}

fn create_dir(path: &Path) -> Result<(), TestSupportError> {
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)
        .map_err(|source| TestSupportError::CreateDir { path: path.to_path_buf(), source })
}

#[cfg(test)]
mod tests;
