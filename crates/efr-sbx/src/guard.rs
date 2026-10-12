//! The surface guard around one contained call (the spec's section 5.6, layer 2).
//!
//! Before the run the launcher records the git surface of every pinned git dir, of
//! every git dir the scan finds in the guard roots and of the start dir's chain; after
//! the run it records it again, compares, and moves every entry that breaks a rule to
//! `$SBX/quarantine/<call>/`, outside the sandbox's view. git itself lists each config
//! (`git config --file - --list --no-includes -z`, with the bytes that the guard read
//! as its input; a config too large to keep, by its path), in `/`, with no system or
//! global config, and from a `PATH` dir that no call can write. A content that git
//! listed before is not listed again: the listings stay in the hidden shell's sandbox
//! dir between calls ([`efr_sandbox::ConfigListings`]).
//!
//! A contained job keeps the record of its start in its state dir, `$SBX/jobs/<call>`,
//! so `efr-sbx guard-check` can compare with it while the job runs, and after a
//! restart. The two hold the job's guard lock while they compare and move entries.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use efr_protocol::SurfaceChange;
use efr_sandbox::{
    CONFIG_LISTINGS_FILE, ConfigLister, ConfigListings, FileKind, FsView, GitDirTarget,
    MAX_CONFIG_LISTINGS_BYTES, MountOp, MountOrigin, MountPlan, SURFACE_BEFORE_FILE, SandboxSpec,
    ScanLimits, SurfaceManifest, check_surface, is_within, scan_git_dirs,
};
use rustix::fs::MemfdFlags;
use serde::{Deserialize, Serialize};

use crate::call_dir::PrivateDir;
use crate::error::SbxError;
use crate::layer_lock::{self, LayerLock, Locked};
use crate::os;
use crate::real_fs::RealFs;

/// The file in a call's quarantine dir that says where each entry came from.
pub(crate) const QUARANTINE_INDEX: &str = "entries.json";

/// Lists a git config with a trusted git.
#[derive(Debug, Clone)]
pub(crate) struct GitLister {
    git: Option<PathBuf>,
}

impl GitLister {
    /// The first `git` on `path` whose directory is absolute and lies in no place that
    /// a call can write (`writable`). A model-written `git` must never run outside.
    pub(crate) fn find(path: &str, writable: &[PathBuf]) -> GitLister {
        let git = path
            .split(':')
            .map(Path::new)
            .filter(|dir| dir.is_absolute() && !writable.iter().any(|root| is_within(dir, root)))
            .map(|dir| dir.join("git"))
            .find(|git| fs::metadata(git).is_ok_and(|meta| meta.is_file()));
        GitLister { git }
    }

    /// What names this git for [`ConfigListings`]: its path and what identifies its
    /// file, so a listing that another git made, or the same path after an update, is
    /// never used. Empty when there is no git.
    pub(crate) fn identity(&self) -> String {
        use std::os::unix::fs::MetadataExt as _;
        let Some(git) = &self.git else { return String::new() };
        let Ok(meta) = fs::metadata(git) else { return String::new() };
        format!(
            "{} {}:{} {} {}.{} {}.{}",
            git.display(),
            meta.dev(),
            meta.ino(),
            meta.size(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec()
        )
    }

    /// `git config --no-includes --list -z --file <file>` in a scrubbed environment, in
    /// `/`, with `stdin` as its input.
    fn run(&self, file: &OsStr, stdin: Option<&[u8]>) -> io::Result<String> {
        let git = self.git.as_ref().ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        // NOTE: the input is a file in memory, not a pipe: git prints the listing while
        // it reads, so a config larger than a pipe would block both ends.
        let input = match stdin {
            Some(bytes) => {
                let memfd = rustix::fs::memfd_create("efr-sbx-config", MemfdFlags::CLOEXEC)?;
                let mut file = fs::File::from(memfd);
                file.write_all(bytes)?;
                file.seek(SeekFrom::Start(0))?;
                Stdio::from(file)
            }
            None => Stdio::null(),
        };
        let output = os::command(git)
            .args(["config", "--no-includes", "--list", "-z", "--file"])
            .arg(file)
            .current_dir("/")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CEILING_DIRECTORIES", "/")
            .env("LC_ALL", "C")
            .stdin(input)
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("git config failed"));
        }
        String::from_utf8(output.stdout).map_err(io::Error::other)
    }
}

impl ConfigLister for GitLister {
    fn list(&self, config: &Path) -> io::Result<String> {
        self.run(config.as_os_str(), None)
    }

    fn list_content(&self, content: &[u8]) -> io::Result<String> {
        self.run(OsStr::new("-"), Some(content))
    }
}

/// The guard of one call.
#[derive(Debug)]
pub(crate) struct Guard {
    pins: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    chain_roots: Vec<PathBuf>,
    protected: Vec<String>,
    lister: GitLister,
    listings: ConfigListings,
    /// The listings file as the call found it.
    kept: Vec<u8>,
    before: SurfaceManifest,
    started: (i64, u32),
}

impl Guard {
    /// Records the surface before the run; `start` is the host path of the start dir,
    /// and `shell_dir` the hidden shell's sandbox dir, which keeps the config listings
    /// of its calls.
    pub(crate) fn before(
        spec: &SandboxSpec,
        plan: &MountPlan,
        start: &Path,
        shell_dir: &PrivateDir,
    ) -> Guard {
        let pins: Vec<PathBuf> = plan
            .mounts()
            .iter()
            .filter(|mount| mount.origin == MountOrigin::Pin)
            .filter_map(|mount| match &mount.op {
                MountOp::Bind { target, .. } => Some(target.clone()),
                _ => None,
            })
            .collect();
        let mut chain_roots: Vec<PathBuf> = plan.write_dirs().to_vec();
        chain_roots.push(plan.private_tmp().to_path_buf());
        let lister = GitLister::find(&spec.shell_path, &chain_roots);
        let git = lister.identity();
        let kept = shell_dir
            .read(CONFIG_LISTINGS_FILE, MAX_CONFIG_LISTINGS_BYTES)
            .ok()
            .flatten()
            .unwrap_or_default();
        let listings = ConfigListings::from_json(&kept, &git);
        let mut guard = Guard {
            pins,
            roots: spec.guard_roots.clone(),
            chain_roots,
            protected: spec.protected_names.clone(),
            lister,
            listings,
            kept,
            before: SurfaceManifest::default(),
            started: now(),
        };
        let targets = guard.targets(start);
        guard.before = SurfaceManifest::capture(
            &targets,
            &guard.protected,
            &RealFs,
            &guard.lister,
            &mut guard.listings,
        );
        guard
    }

    /// Keeps the config listings of this call in `shell_dir` for the next call. A
    /// listing that does not reach the disk costs the next call a run of git, nothing
    /// more.
    pub(crate) fn keep_listings(&self, shell_dir: &PrivateDir) {
        let json = self.listings.to_json();
        if json != self.kept {
            let _ = shell_dir.write_atomic(CONFIG_LISTINGS_FILE, &json);
        }
    }

    /// The directory chain of `cwd` that lies in a place a call can write.
    fn chain(&self, cwd: &Path) -> Vec<PathBuf> {
        cwd.ancestors()
            .filter(|dir| self.chain_roots.iter().any(|root| is_within(dir, root)))
            .map(Path::to_path_buf)
            .collect()
    }

    fn targets(&self, cwd: &Path) -> Vec<GitDirTarget> {
        let chain = self.chain(cwd);
        scan_git_dirs(&self.roots, &chain, &self.pins, &RealFs, ScanLimits::default()).0
    }

    /// Records the surface after the run, with `cwd` the host path of the final dir,
    /// and returns the changes. A git dir that only the second scan finds counts as new
    /// when it or its config, commondir or hooks changed during the call; one that sat
    /// unchanged where the first scan did not look is left out.
    pub(crate) fn after(&mut self, cwd: &Path) -> Vec<SurfaceChange> {
        let known = self.before.git_dirs();
        let targets: Vec<GitDirTarget> = self
            .targets(cwd)
            .into_iter()
            .filter(|target| {
                known.contains(&target.git_dir.as_path()) || self.changed(&target.git_dir)
            })
            .collect();
        let after = SurfaceManifest::capture(
            &targets,
            &self.protected,
            &RealFs,
            &self.lister,
            &mut self.listings,
        );
        check_surface(&self.before, &after)
    }

    fn changed(&self, git_dir: &Path) -> bool {
        let mut paths = vec![
            git_dir.to_path_buf(),
            git_dir.join("config"),
            git_dir.join("config.worktree"),
            git_dir.join("commondir"),
            git_dir.join("hooks"),
        ];
        if let Ok(names) = RealFs.read_dir(&git_dir.join("hooks")) {
            paths.extend(names.into_iter().map(|name| git_dir.join("hooks").join(name)));
        }
        paths
            .iter()
            .filter(|path| RealFs.lstat(path).is_ok())
            .any(|path| RealFs::changed_since(path, self.started))
    }
}

/// The most bytes of a job's [`SURFACE_BEFORE_FILE`].
const MAX_SAVED_BYTES: usize = 64 * 1024 * 1024;

/// How long a job's end guard and `efr-sbx guard-check` wait for each other.
const GUARD_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// A job's guard as it was at the job's start: [`SURFACE_BEFORE_FILE`] in the job's
/// state dir, where no call can reach.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    before: SurfaceManifest,
    started: (i64, u32),
    pins: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    chain_roots: Vec<PathBuf>,
    protected: Vec<String>,
    git: Option<PathBuf>,
    /// The host path of the job's start dir.
    start: PathBuf,
    /// The job's quarantine dir, `$SBX/quarantine/<call>`.
    quarantine: PathBuf,
}

/// What a job's guard needs besides its record: where the job started and where its
/// quarantine is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobPlaces {
    /// The host path of the job's start dir.
    pub(crate) start: PathBuf,
    /// `$SBX/quarantine/<call>`.
    pub(crate) quarantine: PathBuf,
}

impl Guard {
    /// Keeps this guard's record in `dir`, a job's state dir, with the job's places,
    /// for `efr-sbx guard-check`. It lies on disk, so a check after a reboot finds it.
    pub(crate) fn save(&self, dir: &PrivateDir, places: &JobPlaces) -> Result<(), SbxError> {
        let saved = Saved {
            before: self.before.clone(),
            started: self.started,
            pins: self.pins.clone(),
            roots: self.roots.clone(),
            chain_roots: self.chain_roots.clone(),
            protected: self.protected.clone(),
            git: self.lister.git.clone(),
            start: places.start.clone(),
            quarantine: places.quarantine.clone(),
        };
        let bytes = serde_json::to_vec(&saved).map_err(|error| {
            SbxError::os("record the job's git surface", io::Error::other(error))
        })?;
        dir.write_atomic(SURFACE_BEFORE_FILE, &bytes)
    }

    /// The guard of a job from the record in `dir`, its state dir, and the job's places.
    pub(crate) fn load(dir: &PrivateDir) -> Result<(Guard, JobPlaces), SbxError> {
        let path = dir.path().join(SURFACE_BEFORE_FILE);
        let Some(bytes) = dir.read(SURFACE_BEFORE_FILE, MAX_SAVED_BYTES)? else {
            return Err(SbxError::io("read", path, io::ErrorKind::NotFound.into()));
        };
        let saved: Saved = serde_json::from_slice(&bytes)
            .map_err(|error| SbxError::io("read", path, io::Error::other(error)))?;
        let lister = GitLister { git: saved.git };
        let listings = ConfigListings::new(lister.identity());
        let guard = Guard {
            pins: saved.pins,
            roots: saved.roots,
            chain_roots: saved.chain_roots,
            protected: saved.protected,
            lister,
            listings,
            kept: Vec::new(),
            before: saved.before,
            started: saved.started,
        };
        Ok((guard, JobPlaces { start: saved.start, quarantine: saved.quarantine }))
    }
}

/// The guard lock of a job's state dir `dir`: a job's end guard and `efr-sbx
/// guard-check` hold it while they compare and move entries, so neither takes the
/// other's half-moved quarantine as its view. `None` when the other held it for all of
/// [`GUARD_LOCK_WAIT`].
pub(crate) fn lock(dir: &Path) -> Result<Option<LayerLock>, SbxError> {
    match layer_lock::lock(&[dir.to_path_buf()], GUARD_LOCK_WAIT)? {
        Locked::Held(lock) => Ok(Some(lock)),
        Locked::Busy { .. } => Ok(None),
    }
}

fn now() -> (i64, u32) {
    let now = os::since_epoch();
    // A second earlier, so a file made in the second the call began still counts.
    (i64::try_from(now.as_secs()).unwrap_or(i64::MAX).saturating_sub(1), now.subsec_nanos())
}

/// The most bytes that the quarantine of one call copies when an entry lies on another
/// file system than the quarantine dir. A flagged entry is a git config, a hook or a
/// protected file, which is small; the cap keeps a planted giant file from filling the
/// disk.
const MAX_QUARANTINE_COPY: u64 = 64 * 1024 * 1024;

/// One entry of [`QUARANTINE_INDEX`].
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    /// Where it was.
    from: PathBuf,
    /// Its name in the quarantine dir.
    to: String,
    /// The files of the entry, where they were, whose copy keeps only their first
    /// bytes: the copy reached [`MAX_QUARANTINE_COPY`]. efrd does not move such an
    /// entry back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    truncated: Vec<PathBuf>,
}

/// The most bytes of a quarantine index that the launcher reads back.
const MAX_INDEX_BYTES: usize = 1024 * 1024;

/// Moves every quarantined change into `dir` and writes the index; a change that cannot
/// move is reported with `quarantined = false`.
///
/// A rename moves an entry whole. Across file systems the entry is copied and then
/// removed, and the copies of one call share [`MAX_QUARANTINE_COPY`] bytes. A file that
/// does not fit is copied in part, its path goes in the index's `truncated`, and the
/// original is removed all the same: a flagged entry must leave the sandbox's reach,
/// even when part of it is lost.
///
/// A job's quarantine can fill more than once: `efr-sbx guard-check` while the job
/// runs, then the job's own end guard. Each one keeps the entries already there, gives
/// its own a name that is not taken yet, and adds them to the index.
pub(crate) fn quarantine(changes: &mut [SurfaceChange], dir: &Path) -> Result<(), SbxError> {
    if !changes.iter().any(|change| change.quarantined) {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| SbxError::io("create", dir, error))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|error| SbxError::io("protect", dir, error))?;
    let private = PrivateDir::open(dir)?;
    let mut entries: Vec<Entry> = private
        .read(QUARANTINE_INDEX, MAX_INDEX_BYTES)?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut budget = MAX_QUARANTINE_COPY;
    let mut number = entries.len();
    for change in changes.iter_mut().filter(|change| change.quarantined) {
        let name = change.path.file_name().map(OsString::from).unwrap_or_default();
        let to = loop {
            let to = format!("{number}-{}", name.to_string_lossy());
            number += 1;
            if RealFs.lstat(&dir.join(&to)).is_err() {
                break to;
            }
        };
        let mut truncated = Vec::new();
        if move_entry(&change.path, &dir.join(&to), &mut budget, &mut truncated).is_ok() {
            entries.push(Entry { from: change.path.clone(), to, truncated });
        } else {
            change.quarantined = false;
        }
    }
    let index = serde_json::to_vec_pretty(&entries)
        .map_err(|error| SbxError::os("write the quarantine index", io::Error::other(error)))?;
    private.write_atomic(QUARANTINE_INDEX, &index)
}

/// Moves `from`, which must not be reached through a link, to `to`; across file
/// systems by [`move_by_copy`].
fn move_entry(
    from: &Path,
    to: &Path,
    budget: &mut u64,
    truncated: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let parent = from.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let name = from.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let parent_fd =
        RealFs::open(parent, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY)?;
    let target_dir = to.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let target_fd =
        RealFs::open(target_dir, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY)?;
    let target_name = to.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    match rustix::fs::renameat(&parent_fd, name, &target_fd, target_name) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::XDEV) => move_by_copy(from, to, budget, truncated),
        Err(error) => Err(error.into()),
    }
}

/// Copies `from` to `to` with at most `budget` bytes of file content, keeping links as
/// links, then removes `from`. Each file cut short goes in `truncated`.
fn move_by_copy(
    from: &Path,
    to: &Path,
    budget: &mut u64,
    truncated: &mut Vec<PathBuf>,
) -> io::Result<()> {
    copy_tree(from, to, budget, truncated)?;
    if RealFs.lstat(from)? == FileKind::Dir {
        fs::remove_dir_all(from)
    } else {
        fs::remove_file(from)
    }
}

fn copy_tree(
    from: &Path,
    to: &Path,
    budget: &mut u64,
    truncated: &mut Vec<PathBuf>,
) -> io::Result<()> {
    match RealFs.lstat(from)? {
        FileKind::Dir => {
            fs::DirBuilder::new().mode(0o700).create(to)?;
            // Sorted, so the budget runs out at the same file every time.
            let mut names = RealFs.read_dir(from)?;
            names.sort();
            for name in names {
                copy_tree(&from.join(&name), &to.join(&name), budget, truncated)?;
            }
            Ok(())
        }
        FileKind::Symlink => std::os::unix::fs::symlink(fs::read_link(from)?, to),
        FileKind::File => {
            if !copy_file(from, to, budget)? {
                truncated.push(from.to_path_buf());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Streams the regular file `from`, opened with no link on the way, to the new file
/// `to` (mode 0600), with at most `budget` bytes, and takes the bytes copied from
/// `budget`. False when the file had more bytes than that.
fn copy_file(from: &Path, to: &Path, budget: &mut u64) -> io::Result<bool> {
    let source = RealFs::open(from, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK)?;
    let kind = rustix::fs::FileType::from_raw_mode(rustix::fs::fstat(&source)?.st_mode);
    if kind != rustix::fs::FileType::RegularFile {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut target = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(to)?;
    let mut limited = fs::File::from(source).take(*budget);
    let copied = io::copy(&mut limited, &mut target)?;
    *budget = budget.saturating_sub(copied);
    let mut more = [0_u8; 1];
    Ok(limited.into_inner().read(&mut more)? == 0)
}

#[cfg(test)]
mod tests;
