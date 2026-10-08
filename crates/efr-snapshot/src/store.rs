//! Where the store of one root lives: `<dir>/<root-id>.git` (a bare repository),
//! `<root-id>.index` (its persistent index, which keeps git's stat cache, so a later
//! snapshot hashes only changed files) and `<root-id>.root` (the root's canonical path,
//! for the collector). `root-id` is the first 16 hex characters of the SHA-256 of the
//! canonical root path.

use std::fs;
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest as _, Sha256};

use crate::SnapshotError;

/// The most bytes of a project's `.git/info/exclude` that a store copies.
const MAX_EXCLUDE_BYTES: u64 = 64 * 1024;

/// How stale the stamp of a store may get before a snapshot touches it again.
const STAMP_EVERY: Duration = Duration::from_secs(3600);

/// The store of one root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Store {
    dir: PathBuf,
    id: String,
    root: PathBuf,
}

/// The id of `root`, a canonical path: the first 16 hex characters of its SHA-256.
pub(crate) fn root_id(root: &Path) -> String {
    let digest = Sha256::digest(root.as_os_str().as_bytes());
    digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

impl Store {
    /// The store of the canonical `root` in `dir`.
    pub(crate) fn new(dir: &Path, root: &Path) -> Store {
        Store { dir: dir.to_path_buf(), id: root_id(root), root: root.to_path_buf() }
    }

    /// The store with `id` in `dir`, for the collector, which reads the root from the
    /// store's root file.
    pub(crate) fn with_id(dir: &Path, id: &str, root: PathBuf) -> Store {
        Store { dir: dir.to_path_buf(), id: id.to_owned(), root }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn git_dir(&self) -> PathBuf {
        self.dir.join(format!("{}.git", self.id))
    }

    pub(crate) fn index(&self) -> PathBuf {
        self.dir.join(format!("{}.index", self.id))
    }

    pub(crate) fn root_file(&self) -> PathBuf {
        self.dir.join(format!("{}.root", self.id))
    }

    /// The lock that git takes on the persistent index while it writes it.
    pub(crate) fn index_lock(&self) -> PathBuf {
        self.dir.join(format!("{}.index.lock", self.id))
    }

    /// Removes the locks that a killed git left: the index's lock and that of
    /// `packed-refs`. git removes its own lock when it ends, also when it fails, so a
    /// lock stays only when the snapshot timeout or a stop of efrd killed git
    /// (`kill_on_drop`). Each lock blocks every later `git add`, `write-tree` or ref
    /// deletion of the store. The caller holds the store's gate, and efr is the only
    /// writer of the store, so a lock found then belongs to no running git. Returns
    /// true when a lock was removed.
    pub(crate) fn clear_leftover_locks(&self) -> bool {
        let mut removed = false;
        for lock in [self.index_lock(), self.git_dir().join("packed-refs.lock")] {
            removed |= fs::remove_file(&lock).is_ok();
        }
        removed
    }

    /// True when the bare repository exists.
    pub(crate) fn exists(&self) -> bool {
        self.git_dir().join("HEAD").is_file()
    }

    /// Makes the store directory (0700) before `git init` runs in it.
    pub(crate) fn make_dir(&self) -> Result<(), SnapshotError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|source| SnapshotError::Io { path: self.dir.clone(), source })
    }

    /// Writes the root file and the store's `info` directory once the repository
    /// exists.
    pub(crate) fn finish_init(&self) -> Result<(), SnapshotError> {
        let info = self.git_dir().join("info");
        fs::create_dir_all(&info).map_err(|source| SnapshotError::Io { path: info, source })?;
        let file = self.root_file();
        efr_stdx::fs::write_atomic(&file, self.root.as_os_str().as_bytes()).map_err(|source| {
            SnapshotError::Io { path: file.clone(), source: std::io::Error::other(source) }
        })
    }

    /// The number of entries in the persistent index, read from its header; `None`
    /// when there is no index yet.
    pub(crate) fn index_entries(&self) -> Option<u64> {
        let mut header = [0_u8; 12];
        let mut file = fs::File::open(self.index()).ok()?;
        file.read_exact(&mut header).ok()?;
        if &header[..4] != b"DIRC" {
            return None;
        }
        Some(u64::from(u32::from_be_bytes([header[8], header[9], header[10], header[11]])))
    }

    /// Marks the store as used now, at most once an hour, so the collector counts its
    /// idle days from the last snapshot.
    pub(crate) fn stamp(&self, now: SystemTime) {
        let file = self.root_file();
        let stale = fs::metadata(&file)
            .and_then(|metadata| metadata.modified())
            // NOTE: a time after `now` is stale too, so the stamp follows the injected
            // clock.
            .map(|modified| now.duration_since(modified).map_or(true, |age| age >= STAMP_EVERY))
            .unwrap_or(true);
        if stale && let Ok(handle) = fs::File::options().write(true).open(&file) {
            let _ = handle.set_modified(now);
        }
    }

    /// Copies the root's own `.git/info/exclude` (at most 64 KiB, never through a
    /// symbolic link) to the store's `info/exclude`, because with the store as
    /// `GIT_DIR` git reads the store's file, not the project's. A root without one
    /// gets an empty file.
    pub(crate) fn copy_exclude(&self) -> Result<(), SnapshotError> {
        let wanted = project_exclude(&self.root).unwrap_or_default();
        let target = self.git_dir().join("info").join("exclude");
        if fs::read(&target).is_ok_and(|current| current == wanted) {
            return Ok(());
        }
        efr_stdx::fs::write_atomic(&target, &wanted).map_err(|source| SnapshotError::Io {
            path: target.clone(),
            source: std::io::Error::other(source),
        })
    }
}

/// The project's `info/exclude` below the git directory of `root`, read without
/// following a symbolic link at any step: `.git` must be a directory, or a file that
/// names one with `gitdir:` (a linked work tree, whose exclude file lives in the common
/// directory).
fn project_exclude(root: &Path) -> Option<Vec<u8>> {
    let dot_git = root.join(".git");
    let metadata = fs::symlink_metadata(&dot_git).ok()?;
    let git_dir = if metadata.is_dir() {
        dot_git
    } else if metadata.is_file() {
        let text = read_small(&dot_git, 4096)?;
        let text = String::from_utf8(text).ok()?;
        let named = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        let named = if named.is_absolute() { named } else { root.join(named) };
        let common = read_small(&named.join("commondir"), 4096)
            .and_then(|text| String::from_utf8(text).ok())
            .map(|text| PathBuf::from(text.trim()));
        match common {
            Some(common) if common.is_absolute() => common,
            Some(common) => named.join(common),
            None => named,
        }
    } else {
        return None;
    };
    let info = git_dir.join("info");
    if !fs::symlink_metadata(&info).ok()?.is_dir() {
        return None;
    }
    read_small(&info.join("exclude"), MAX_EXCLUDE_BYTES)
}

/// At most `limit` bytes of the regular file at `path`, never through a symbolic link:
/// the file that was opened must be the one that `lstat` saw, so a link swapped in
/// between the two is refused.
fn read_small(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let seen = fs::symlink_metadata(path).ok()?;
    if !seen.is_file() {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let opened = file.metadata().ok()?;
    if (opened.dev(), opened.ino()) != (seen.dev(), seen.ino()) {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The `HEAD` of the project at `root`, read from `.git/HEAD` directly, for the
/// message of a turn's first commit; `None` when the root is no work tree with a
/// `.git` directory.
pub(crate) fn project_head(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    if !fs::symlink_metadata(&dot_git).ok()?.is_dir() {
        return None;
    }
    let head = read_small(&dot_git.join("HEAD"), 4096)?;
    String::from_utf8(head).ok().map(|head| head.trim().to_owned())
}

#[cfg(test)]
mod tests;
