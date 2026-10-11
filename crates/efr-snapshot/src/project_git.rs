//! [`ProjectGit`]: git on a project's own git dir, in a second hardened mode of the
//! runner. The dirs come from the daemon ([`GitDirs`]), which trusts them through its
//! work tree record; this crate never follows a `.git` file of a root to find them.
//! Both dirs are pinned by their `(dev, ino)`, and a repository with alternates, or a
//! main git dir with a `commondir`, is refused.
//!
//! Each command runs with the project's git dir as `GIT_DIR`, no work tree and no
//! index of the store, no system or global config, and `-c` settings that switch off
//! everything in the project's config that could run code: hooks (also the
//! `reference-transaction` hook), fsmonitor, the alternate refs command, automatic gc
//! and maintenance, with bounded packing. Only these commands run: `for-each-ref`,
//! `symbolic-ref` (read), `update-ref` (compare and swap), `cat-file -e`,
//! `index-pack --stdin --fix-thin`, `diff-index --cached --quiet HEAD`,
//! `rev-list --walk-reflogs --count refs/stash`, and `upload-pack` as the
//! `--upload-pack` of a fetch into the store ([`ProjectGit::upload_pack`]).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crate::SnapshotError;
use crate::runner::{Output, Runner};

/// The `-c` settings of every command on a project's git dir. They go into the
/// `--upload-pack` string too, because the `-c` of a fetch does not reach a local
/// `upload-pack`.
const PROJECT_HARDENING: &[&str] = &[
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    "core.alternateRefsCommand=",
    "gc.auto=0",
    "maintenance.auto=false",
    "pack.threads=1",
    "pack.window=10",
    "pack.depth=50",
    "core.bigFileThreshold=512m",
    "core.packedGitLimit=256m",
];

/// The git dirs of a project, as the daemon trusts them: from its work tree record when
/// the record matches, else `<root>/.git` when that is a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// The git dir of the work tree: `<root>/.git`, or `<common>/worktrees/<name>` for
    /// a linked work tree.
    pub git_dir: PathBuf,
    /// The common dir, which holds the objects and the shared refs; the git dir itself
    /// for a main work tree.
    pub common_dir: PathBuf,
}

/// The `(dev, ino)` of both git dirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pins {
    pub(crate) git_dir: (u64, u64),
    pub(crate) common_dir: (u64, u64),
}

/// Why efr runs no git on a project's git dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A git dir is no directory, or it is a link.
    NotADir,
    /// A git dir is not the one that the turn's first snapshot pinned.
    Changed,
    /// The objects of the project live in part in another repository.
    Alternates,
    /// A main git dir names a common dir, or a linked git dir names another one.
    CommonDir,
}

impl Refusal {
    /// Why, for the record and the report.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Refusal::NotADir => "the git dir is not a directory",
            Refusal::Changed => "the git dir changed",
            Refusal::Alternates => "alternates",
            Refusal::CommonDir => "a commondir file",
        }
    }
}

/// git on one project's git dir.
#[derive(Debug, Clone)]
pub(crate) struct ProjectGit {
    runner: Runner,
    dirs: GitDirs,
    pins: Pins,
}

/// The project's refs that a mirror takes, and its `HEAD`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectRefs {
    /// `refs/heads/*`, `refs/tags/*` and `refs/stash`: name to object id.
    pub(crate) refs: BTreeMap<String, String>,
    /// The branch that `HEAD` names, such as `refs/heads/main`.
    pub(crate) head_ref: Option<String>,
    /// The commit of a detached `HEAD`.
    pub(crate) head_commit: Option<String>,
}

impl ProjectGit {
    /// git on `dirs`, after the checks of this module; with `expected`, both dirs must
    /// have those `(dev, ino)`. Reads the file system: run it on the blocking pool.
    pub(crate) fn open(
        runner: &Runner,
        dirs: &GitDirs,
        expected: Option<Pins>,
    ) -> Result<ProjectGit, Refusal> {
        let pin = |dir: &Path| {
            let metadata = fs::symlink_metadata(dir).map_err(|_| Refusal::NotADir)?;
            if !metadata.is_dir() {
                return Err(Refusal::NotADir);
            }
            Ok((metadata.dev(), metadata.ino()))
        };
        let pins = Pins { git_dir: pin(&dirs.git_dir)?, common_dir: pin(&dirs.common_dir)? };
        if expected.is_some_and(|expected| expected != pins) {
            return Err(Refusal::Changed);
        }
        if fs::symlink_metadata(dirs.common_dir.join("objects/info/alternates")).is_ok() {
            return Err(Refusal::Alternates);
        }
        let named = fs::read_to_string(dirs.git_dir.join("commondir")).ok();
        match named {
            None if pins.git_dir == pins.common_dir => {}
            Some(_) if pins.git_dir == pins.common_dir => return Err(Refusal::CommonDir),
            None => return Err(Refusal::CommonDir),
            Some(named) => {
                let named = dirs.git_dir.join(named.trim());
                let same = fs::metadata(&named)
                    .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == pins.common_dir);
                if !same {
                    return Err(Refusal::CommonDir);
                }
            }
        }
        Ok(ProjectGit { runner: runner.clone(), dirs: dirs.clone(), pins })
    }

    pub(crate) fn pins(&self) -> Pins {
        self.pins
    }

    pub(crate) fn dirs(&self) -> &GitDirs {
        &self.dirs
    }

    /// True when the project is a shallow clone, whose history the store cannot take.
    pub(crate) fn shallow(&self) -> bool {
        fs::symlink_metadata(self.dirs.common_dir.join("shallow")).is_ok()
    }

    /// The size of the project's object dir, from an `lstat` walk that follows no
    /// link; it stops once it passes `most`.
    pub(crate) fn objects_bytes(&self, most: u64) -> u64 {
        let mut total = 0_u64;
        let objects = self.dirs.common_dir.join("objects");
        if !fs::symlink_metadata(&objects).is_ok_and(|metadata| metadata.is_dir()) {
            return 0;
        }
        let mut stack = vec![objects];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.filter_map(Result::ok) {
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    stack.push(entry.path());
                } else if kind.is_file() {
                    total = total.saturating_add(entry.metadata().map_or(0, |meta| meta.len()));
                    if total > most {
                        return total;
                    }
                }
            }
        }
        total
    }

    /// The `--upload-pack` of a fetch from the project into the store.
    pub(crate) fn upload_pack() -> String {
        let mut command = String::from("git");
        for setting in PROJECT_HARDENING {
            command.push_str(" -c ");
            command.push_str(setting);
        }
        command.push_str(" upload-pack");
        command
    }

    /// The refs that a mirror takes, and `HEAD`.
    pub(crate) async fn refs(&self) -> Result<ProjectRefs, SnapshotError> {
        let out = self
            .checked(
                "for-each-ref",
                &[
                    "for-each-ref",
                    "--format=%(objectname) %(refname)",
                    "refs/heads/",
                    "refs/tags/",
                    "refs/stash",
                ],
            )
            .await?;
        let mut found = ProjectRefs::default();
        for line in String::from_utf8_lossy(&out).lines() {
            if let Some((id, name)) = line.split_once(' ') {
                found.refs.insert(name.to_owned(), id.to_owned());
            }
        }
        let head = self.run(&["symbolic-ref", "-q", "HEAD"], None).await?;
        if head.success {
            found.head_ref = Some(String::from_utf8_lossy(&head.stdout).trim().to_owned());
        } else {
            // NOTE: a detached HEAD names its commit in the file; for-each-ref does
            // not list HEAD on every git version.
            let dir = self.dirs.git_dir.clone();
            let text = self.runner.blocking(&dir.clone(), move || read_head(&dir)).await?;
            found.head_commit = text.filter(|text| {
                text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
        }
        Ok(found)
    }

    /// True when the project's index differs from `HEAD`: content that is only staged.
    pub(crate) async fn staged(&self) -> Result<bool, SnapshotError> {
        let output = self.run(&["diff-index", "--cached", "--quiet", "HEAD", "--"], None).await?;
        match output.code {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(self.failed("diff-index")),
        }
    }

    /// The entries of the project's stash.
    pub(crate) async fn stash_entries(&self) -> Result<u32, SnapshotError> {
        let out = self
            .checked("rev-list", &["rev-list", "--walk-reflogs", "--count", "refs/stash"])
            .await?;
        String::from_utf8_lossy(&out)
            .trim()
            .parse()
            .map_err(|_| SnapshotError::BadOutput { command: "rev-list" })
    }

    /// True when the project holds the object `id`.
    #[cfg_attr(not(test), expect(dead_code, reason = "undo moves the refs (phase B, P4)"))]
    pub(crate) async fn has_object(&self, id: &str) -> Result<bool, SnapshotError> {
        Ok(self.run(&["cat-file", "-e", id], None).await?.success)
    }

    /// Moves the ref `name` from `from` to `to` (`None`: absent) when it still has
    /// `from`.
    #[cfg_attr(not(test), expect(dead_code, reason = "undo moves the refs (phase B, P4)"))]
    pub(crate) async fn update_ref(
        &self,
        name: &str,
        to: Option<&str>,
        from: Option<&str>,
    ) -> Result<bool, SnapshotError> {
        let zero = "0".repeat(40);
        let line = match to {
            Some(to) => format!("update {name} {to} {}\n", from.unwrap_or(&zero)),
            None => format!("delete {name} {}\n", from.unwrap_or(&zero)),
        };
        Ok(self.run(&["update-ref", "--stdin"], Some(line.into_bytes())).await?.success)
    }

    /// Takes the pack `pack` into the project's objects.
    #[cfg_attr(not(test), expect(dead_code, reason = "undo moves the objects (phase B, P4)"))]
    pub(crate) async fn index_pack(&self, pack: Vec<u8>) -> Result<(), SnapshotError> {
        self.checked_input("index-pack", &["index-pack", "--stdin", "--fix-thin"], pack).await?;
        Ok(())
    }

    async fn checked(
        &self,
        command: &'static str,
        args: &[&str],
    ) -> Result<Vec<u8>, SnapshotError> {
        let output = self.run(args, None).await?;
        if output.success { Ok(output.stdout) } else { Err(self.failed(command)) }
    }

    async fn checked_input(
        &self,
        command: &'static str,
        args: &[&str],
        stdin: Vec<u8>,
    ) -> Result<Vec<u8>, SnapshotError> {
        let output = self.run(args, Some(stdin)).await?;
        if output.success { Ok(output.stdout) } else { Err(self.failed(command)) }
    }

    fn failed(&self, command: &'static str) -> SnapshotError {
        SnapshotError::GitFailed { command, store: self.dirs.git_dir.clone() }
    }

    async fn run(&self, args: &[&str], stdin: Option<Vec<u8>>) -> Result<Output, SnapshotError> {
        let mut all: Vec<OsString> = Vec::new();
        for setting in PROJECT_HARDENING {
            all.push("-c".into());
            all.push((*setting).into());
        }
        all.extend(args.iter().map(OsString::from));
        let mut command = self.runner.bare_command(&self.dirs.git_dir, &all);
        command
            .env("GIT_DIR", &self.dirs.git_dir)
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_LITERAL_PATHSPECS")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        self.runner.execute(command, stdin).await
    }
}

/// The text of the `HEAD` file of the git dir `dir`, never through a link.
fn read_head(dir: &Path) -> Option<String> {
    let path = dir.join("HEAD");
    if !fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    let text = fs::read(&path).ok()?;
    String::from_utf8(text.into_iter().take(4096).collect()).ok().map(|text| text.trim().to_owned())
}

#[cfg(test)]
mod tests;
