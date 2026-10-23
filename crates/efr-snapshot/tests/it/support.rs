//! A store in a temporary directory, isolated git, and a few file helpers.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, TurnId};
use efr_scope::{Git, Home};
use efr_snapshot::{DEFAULT_SNAPSHOT_TIMEOUT, GitDirs, Limits, Root, SnapshotParts, Snapshots};
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
        World::in_base(tempfile::tempdir().unwrap())
    }

    /// A world below cargo's target temp dir, not on the tmpfs of `/tmp`, for a large
    /// fixture.
    pub(crate) fn in_target() -> World {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("snapshot-worlds");
        fs::create_dir_all(&dir).unwrap();
        World::in_base(tempfile::tempdir_in(dir).unwrap())
    }

    fn in_base(base: tempfile::TempDir) -> World {
        fs::create_dir_all(base.path().join("home/.config")).unwrap();
        fs::create_dir_all(base.path().join("data")).unwrap();
        let clock = TestClock::starting_at("2026-10-08T10:00:00Z".parse::<Timestamp>().unwrap());
        let snapshots = store(base.path(), &clock);
        World { base, clock, snapshots }
    }

    /// A world whose store runs git through a script that writes the subcommand of each
    /// run to a log ([`World::git_runs`]).
    pub(crate) fn with_git_log() -> World {
        World::new().logged()
    }

    /// This world with its store running git through the logging script of
    /// [`World::with_git_log`].
    pub(crate) fn logged(self) -> World {
        let world = self;
        let script = world.base.path().join("git-logged");
        let log = world.base.path().join("git-runs.log");
        let text = format!(
            "#!/bin/sh\n\
             skip=\n\
             for arg; do\n\
             \x20 if [ -n \"$skip\" ]; then skip=; elif [ \"$arg\" = -c ]; then skip=1; \
             else printf '%s\\n' \"$arg\" >> '{}'; break; fi\n\
             done\n\
             exec git \"$@\"\n",
            log.display()
        );
        world.with_git_script(&script, &text)
    }

    /// A world whose store runs git through the shell script `text` at `script`.
    pub(crate) fn with_git_script(self, script: &Path, text: &str) -> World {
        fs::write(script, text).unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
        let home = Home::new(self.home()).unwrap();
        let snapshots = Snapshots::new(SnapshotParts {
            dir: self.store_dir(),
            git: Git::new(self.clock.shared()).isolated().with_program(script),
            home,
            clock: self.clock.shared(),
            excludes_file: None,
            timeout: DEFAULT_SNAPSHOT_TIMEOUT,
        });
        World { snapshots, ..self }
    }

    /// The git subcommands that the store ran since the last look, sorted.
    pub(crate) fn git_runs(&self) -> Vec<String> {
        let log = self.base.path().join("git-runs.log");
        let text = fs::read_to_string(&log).unwrap_or_default();
        let _ = fs::remove_file(&log);
        let mut runs: Vec<String> = text.lines().map(str::to_owned).collect();
        runs.sort();
        runs
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
    Limits { max_file_bytes: 1024, ..Limits::default() }
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

/// `git ls-tree -r` of `rev` in the store at `git_dir`: path to blob id.
pub(crate) async fn blobs_of(git_dir: &Path, home: &Path, rev: &str) -> BTreeMap<String, String> {
    let out = git(home, home, &[("GIT_DIR", git_dir)], &["ls-tree", "-r", rev]).await;
    out.lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            Some((path.to_owned(), meta.split(' ').nth(2)?.to_owned()))
        })
        .collect()
}

/// The blob ids of the persistent index of the store at `git_dir`: path to blob id.
pub(crate) async fn index_blobs(git_dir: &Path, home: &Path) -> BTreeMap<String, String> {
    let index = git_dir.with_extension("index");
    let env = [("GIT_DIR", git_dir), ("GIT_INDEX_FILE", index.as_path())];
    let out = git(home, home, &env, &["ls-files", "-s"]).await;
    out.lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            Some((path.to_owned(), meta.split(' ').nth(1)?.to_owned()))
        })
        .collect()
}

/// The `efr-meta:` line of the commit `rev` in the store at `git_dir`.
pub(crate) async fn meta_of(git_dir: &Path, home: &Path, rev: &str) -> serde_json::Value {
    let text = git(home, home, &[("GIT_DIR", git_dir)], &["cat-file", "commit", rev]).await;
    let line = text.lines().find_map(|line| line.strip_prefix("efr-meta: ")).unwrap();
    serde_json::from_str(line).unwrap()
}

/// The `(path, reason)` pairs of the `left_out` record of `meta`.
pub(crate) fn left_out(meta: &serde_json::Value) -> Vec<(String, String)> {
    meta["left_out"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().unwrap().to_owned(),
                entry["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A git project below the home directory with one commit of `a.txt`.
pub(crate) async fn project(world: &World, name: &str) -> PathBuf {
    let home = world.home();
    let dir = world.dir(name);
    write(&dir.join("a.txt"), "a\n");
    git(&dir, &home, &[], &["init", "-q"]).await;
    git(&dir, &home, &[], &["add", "a.txt"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "one"]).await;
    dir
}

/// The git dirs of the main work tree at `dir`, as the daemon trusts them.
pub(crate) fn dirs_of(dir: &Path) -> GitDirs {
    GitDirs { git_dir: dir.join(".git"), common_dir: dir.join(".git") }
}

/// The root at `dir` with the git dirs of its main work tree.
pub(crate) fn git_root(dir: &Path) -> Root {
    Root::new(dir, "").with_git(Some(dirs_of(dir)))
}

/// Every ref of the store at `git_dir`, sorted.
pub(crate) async fn refs(git_dir: &Path, home: &Path) -> Vec<String> {
    let out =
        git(home, home, &[("GIT_DIR", git_dir)], &["for-each-ref", "--format=%(refname)"]).await;
    out.lines().map(str::to_owned).collect()
}

/// The ref `refs/efr/<conversation 1>/<turn n>/<name>`.
pub(crate) fn turn_ref(n: u128, name: &str) -> String {
    format!("refs/efr/{}/{}/{name}", conversation(1), turn(n))
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
