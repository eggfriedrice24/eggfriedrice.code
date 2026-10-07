//! What the plan does with one path (`sandbox.explain`), and where a call starts.

use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::SandboxPathRole;
use serde::{Deserialize, Serialize};

use crate::fs_view::{FileKind, FsView};
use crate::paths::is_within;
use crate::plan::{MountOrigin, MountPlan};
use crate::state::SandboxCwd;

/// What the sandbox does with one path, from the mount that decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Explanation {
    /// The role, as `sandbox.explain` reports it.
    pub role: SandboxPathRole,
    /// The mount that decides it; `None` for the read-only rest of the system.
    pub origin: Option<MountOrigin>,
    /// True when a contained call reads the real content.
    pub read: bool,
    /// True when a contained call can write it.
    pub write: bool,
}

/// Where a contained call starts (the spec's section 3.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StartDir {
    /// The hidden shell's working directory.
    Here {
        /// The directory.
        path: PathBuf,
    },
    /// The sandbox's own directory in the private tmp, where the last call ended.
    Private {
        /// The directory, below `/tmp` or `/var/tmp` inside the sandbox.
        path: PathBuf,
    },
    /// `$SCRATCH`, because the shell's directory is hidden in the sandbox; the tool
    /// result says so.
    Scratch {
        /// `$SCRATCH`.
        path: PathBuf,
        /// The hidden directory.
        hidden: PathBuf,
    },
}

impl StartDir {
    /// The directory bwrap's `--chdir` names.
    pub fn path(&self) -> &Path {
        match self {
            StartDir::Here { path }
            | StartDir::Private { path }
            | StartDir::Scratch { path, .. } => path,
        }
    }
}

/// True when `path` lies below the host's `/tmp` or `/var/tmp`, which the private tmp
/// replaces.
pub(crate) fn in_host_tmp(path: &Path) -> bool {
    is_within(path, Path::new("/tmp")) || is_within(path, Path::new("/var/tmp"))
}

impl MountPlan {
    /// What a contained call can do with `path`, an absolute path in normal form with
    /// its links already followed: the last mount whose target covers it decides.
    pub fn explain(&self, path: &Path) -> Explanation {
        let origin = self
            .mounts
            .iter()
            .rfind(|mount| is_within(path, mount.op.target()))
            .map(|mount| mount.origin);
        let role = match origin {
            None | Some(MountOrigin::Socket | MountOrigin::Device) => SandboxPathRole::ReadOnly,
            Some(MountOrigin::Runtime | MountOrigin::EfrState | MountOrigin::Mask(_)) => {
                SandboxPathRole::Masked
            }
            Some(MountOrigin::Cache) => SandboxPathRole::CacheOverlay,
            Some(MountOrigin::WriteRoot(_) | MountOrigin::Pin) => SandboxPathRole::WriteRoot,
            Some(MountOrigin::PrivateTmp | MountOrigin::SharedMemory) => {
                SandboxPathRole::PrivateTmp
            }
            Some(MountOrigin::Floor(_) | MountOrigin::Asset) => SandboxPathRole::Floor,
        };
        let read = role != SandboxPathRole::Masked;
        let write = matches!(
            role,
            SandboxPathRole::WriteRoot
                | SandboxPathRole::CacheOverlay
                | SandboxPathRole::PrivateTmp
        );
        Explanation { role, origin, read, write }
    }

    /// Where a call starts when the hidden shell is in `shell_pwd`: the sandbox's own
    /// directory in the private tmp when the shell did not move since the last call
    /// ended there and the directory still exists, else `shell_pwd`, else `$SCRATCH`
    /// when `shell_pwd` is masked or lies in the host's `/tmp`.
    pub fn start_dir(
        &self,
        shell_pwd: &Path,
        last: Option<&SandboxCwd>,
        scratch: &Path,
        fs: &dyn FsView,
    ) -> StartDir {
        if let Some(last) = last
            && last.shell_pwd == shell_pwd
            && let Some(host) = self.private_host_path(&last.path)
            && fs.lstat(&host).map_err(|error| error.kind()) == Ok(FileKind::Dir)
        {
            return StartDir::Private { path: last.path.clone() };
        }
        let hidden =
            in_host_tmp(shell_pwd) || self.explain(shell_pwd).role == SandboxPathRole::Masked;
        let missing =
            fs.lstat(shell_pwd).map_err(|error| error.kind()) == Err(io::ErrorKind::NotFound);
        if hidden || missing {
            return StartDir::Scratch {
                path: scratch.to_path_buf(),
                hidden: shell_pwd.to_path_buf(),
            };
        }
        StartDir::Here { path: shell_pwd.to_path_buf() }
    }

    /// The host path of `inside`, a path below the private `/tmp` or `/var/tmp`.
    pub fn private_host_path(&self, inside: &Path) -> Option<PathBuf> {
        ["/tmp", "/var/tmp"]
            .into_iter()
            .find_map(|root| inside.strip_prefix(root).ok().map(|rest| self.private_tmp.join(rest)))
    }
}
