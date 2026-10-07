//! A store in a temporary directory, isolated git, and a few file helpers.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, TurnId};
use efr_scope::{Git, Home};
use efr_snapshot::{DEFAULT_SNAPSHOT_TIMEOUT, Limits, Root, SnapshotParts, Snapshots};
use efr_test_support::TestClock;
use jiff::Timestamp;

/// A home, a store and the clock of one test.
pub(crate) struct World {
    pub(crate) base: tempfile::TempDir,
    pub(crate) clock: TestClock,
    pub(crate) snapshots: Snapshots,
}

impl World {
    pub(crate) fn new() -> World {
        let base = tempfile::tempdir().unwrap();
        fs::create_dir_all(base.path().join("home/.config")).unwrap();
        fs::create_dir_all(base.path().join("data")).unwrap();
        let clock = TestClock::starting_at("2026-10-08T10:00:00Z".parse::<Timestamp>().unwrap());
        let snapshots = store(base.path(), &clock);
        World { base, clock, snapshots }
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.base.path().join("home")
    }

    /// The store directory, `$D/snapshots`.
    pub(crate) fn store_dir(&self) -> PathBuf {
        self.base.path().join("data/snapshots")
    }

    /// A new directory below the home directory.
    pub(crate) fn dir(&self, name: &str) -> PathBuf {
        let dir = self.home().join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    /// The only store's git directory.
    pub(crate) fn only_store(&self) -> PathBuf {
        let stores = stores(&self.store_dir());
        assert_eq!(stores.len(), 1, "{stores:?}");
        stores.into_iter().next().unwrap()
    }
}

/// A store in `base/data/snapshots` with an isolated git.
pub(crate) fn store(base: &Path, clock: &TestClock) -> Snapshots {
    let home = Home::new(base.join("home")).unwrap();
    Snapshots::new(SnapshotParts {
        dir: base.join("data/snapshots"),
        git: Git::new(clock.shared()).isolated(),
        home,
        clock: clock.shared(),
        excludes_file: None,
        timeout: DEFAULT_SNAPSHOT_TIMEOUT,
    })
}

/// Every store's git directory below `dir`.
pub(crate) fn stores(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "git"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

pub(crate) fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid(0x0100 + n))
}

pub(crate) fn turn(n: u128) -> TurnId {
    TurnId::from_uuid(uuid(0x0200 + n))
}

/// A UUIDv7-shaped id whose text sorts by `n`.
fn uuid(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(0x019a_9b1c_3d00_7a10_8b20_0000_0000_0000 | n)
}

/// The limits of the tests: untracked files up to 1 KiB.
pub(crate) fn limits() -> Limits {
    Limits { max_file_bytes: 1024, ignored_small: true, max_files: 20_000 }
}

pub(crate) fn root(path: &Path) -> Root {
    Root::new(path, "")
}

pub(crate) fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

/// Runs git with no user config in `cwd`, with `env` added, and returns its output.
pub(crate) async fn git(cwd: &Path, home: &Path, env: &[(&str, &Path)], args: &[&str]) -> String {
    let mut command = efr_stdx::process::command("git", cwd);
    command
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().await.unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

/// `git ls-tree -r` of `rev` in the store at `git_dir`: path to mode.
pub(crate) async fn tree_of(git_dir: &Path, home: &Path, rev: &str) -> BTreeMap<String, String> {
    let out = git(home, home, &[("GIT_DIR", git_dir)], &["ls-tree", "-r", rev]).await;
    out.lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            Some((path.to_owned(), meta.split(' ').next()?.to_owned()))
        })
        .collect()
}

/// Every file below `dir` with its bytes, for a comparison before and after.
pub(crate) fn all_bytes(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in fs::read_dir(&at).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                found.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    found
}
