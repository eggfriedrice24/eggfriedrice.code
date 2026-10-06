//! The surface guard around one contained call (the spec's section 5.6, layer 2).
//!
//! Before the run the launcher records the git surface of every pinned git dir, of
//! every git dir the scan finds in the guard roots and of the start dir's chain; after
//! the run it records it again, compares, and moves every entry that breaks a rule to
//! `$SBX/quarantine/<call>/`, outside the sandbox's view. git itself lists each config
//! (`git config --file F --list --no-includes -z`), in `/`, with no system or global
//! config, and from a `PATH` dir that no call can write.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use efr_protocol::SurfaceChange;
use efr_sandbox::{
    ConfigLister, FileKind, FsView, GitDirTarget, MountOp, MountOrigin, MountPlan, SandboxSpec,
    ScanLimits, SurfaceManifest, check_surface, is_within, scan_git_dirs,
};
use serde::Serialize;

use crate::call_dir::PrivateDir;
use crate::error::SbxError;
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
}

impl ConfigLister for GitLister {
    fn list(&self, config: &Path) -> io::Result<String> {
        let git = self.git.as_ref().ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let output = os::command(git)
            .args(["config", "--no-includes", "--list", "-z", "--file"])
            .arg(config)
            .current_dir("/")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CEILING_DIRECTORIES", "/")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("git config failed"));
        }
        String::from_utf8(output.stdout).map_err(io::Error::other)
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
    before: SurfaceManifest,
    started: (i64, u32),
}

impl Guard {
    /// Records the surface before the run; `start` is the host path of the start dir.
    pub(crate) fn before(spec: &SandboxSpec, plan: &MountPlan, start: &Path) -> Guard {
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
        let mut guard = Guard {
            pins,
            roots: spec.guard_roots.clone(),
            chain_roots,
            protected: spec.protected_names.clone(),
            lister,
            before: SurfaceManifest::default(),
            started: now(),
        };
        let targets = guard.targets(start);
        guard.before = SurfaceManifest::capture(&targets, &guard.protected, &RealFs, &guard.lister);
        guard
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
    pub(crate) fn after(&self, cwd: &Path) -> Vec<SurfaceChange> {
        let known = self.before.git_dirs();
        let targets: Vec<GitDirTarget> = self
            .targets(cwd)
            .into_iter()
            .filter(|target| {
                known.contains(&target.git_dir.as_path()) || self.changed(&target.git_dir)
            })
            .collect();
        let after = SurfaceManifest::capture(&targets, &self.protected, &RealFs, &self.lister);
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

fn now() -> (i64, u32) {
    let now = os::since_epoch();
    // A second earlier, so a file made in the second the call began still counts.
    (i64::try_from(now.as_secs()).unwrap_or(i64::MAX).saturating_sub(1), now.subsec_nanos())
}

/// One entry of [`QUARANTINE_INDEX`].
#[derive(Debug, Serialize)]
struct Entry<'a> {
    /// Where it was.
    from: &'a Path,
    /// Its name in the quarantine dir.
    to: String,
}

/// Moves every quarantined change into `dir` and writes the index; a change that cannot
/// move is reported with `quarantined = false`.
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
    let mut entries = Vec::new();
    for (at, change) in changes.iter_mut().enumerate().filter(|(_, change)| change.quarantined) {
        let name = change.path.file_name().map(OsString::from).unwrap_or_default();
        let to = format!("{at}-{}", name.to_string_lossy());
        if move_entry(&change.path, &dir.join(&to)).is_ok() {
            entries.push(Entry { from: &change.path, to });
        } else {
            change.quarantined = false;
        }
    }
    let index = serde_json::to_vec_pretty(&entries)
        .map_err(|error| SbxError::os("write the quarantine index", io::Error::other(error)))?;
    PrivateDir::open(dir)?.write_atomic(QUARANTINE_INDEX, &index)
}

/// Moves `from`, which must not be reached through a link, to `to`; across file
/// systems by a copy that keeps links as links, then a removal.
fn move_entry(from: &Path, to: &Path) -> io::Result<()> {
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
        Err(rustix::io::Errno::XDEV) => {
            copy_tree(from, to)?;
            if RealFs.lstat(from)? == FileKind::Dir {
                fs::remove_dir_all(from)
            } else {
                fs::remove_file(from)
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    match RealFs.lstat(from)? {
        FileKind::Dir => {
            fs::DirBuilder::new().mode(0o700).create(to)?;
            for name in RealFs.read_dir(from)? {
                copy_tree(&from.join(&name), &to.join(&name))?;
            }
            Ok(())
        }
        FileKind::Symlink => std::os::unix::fs::symlink(fs::read_link(from)?, to),
        FileKind::File => {
            let bytes = RealFs.read_file(from, usize::MAX >> 1)?;
            fs::write(to, bytes)?;
            fs::set_permissions(to, fs::Permissions::from_mode(0o600))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
