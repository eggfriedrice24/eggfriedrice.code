//! The bwrap argument list and the descriptors behind it.
//!
//! The launcher writes the list NUL-separated to a pipe and passes it with `--args`,
//! so `/proc/<pid>/cmdline` of bwrap shows almost nothing. The list holds paths, names
//! and descriptor numbers only, never a value of an environment variable: the launcher
//! passes the environment to bwrap directly and never uses `--setenv`.

use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

use crate::SandboxError;
use crate::fs_view::FsView;
use crate::plan::{MountOp, MountPlan};

/// The descriptors behind the bind operations of one launch.
///
/// Each `--bind-fd` and `--ro-bind-fd` gets a descriptor of its own: bwrap checks the
/// descriptor after the mount and closes it, so a second bind from the same one fails
/// with "Can't stat fd". The table never hands out one descriptor twice.
pub struct FdTable<'a> {
    fs: &'a dyn FsView,
    fds: Vec<(OwnedFd, PathBuf)>,
}

impl std::fmt::Debug for FdTable<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FdTable").field("fds", &self.fds).finish_non_exhaustive()
    }
}

impl<'a> FdTable<'a> {
    /// An empty table that opens through `fs`.
    pub fn new(fs: &'a dyn FsView) -> Self {
        FdTable { fs, fds: Vec::new() }
    }

    /// Opens `path` with no symbolic link on the way and returns the new descriptor's
    /// number.
    pub fn open(&mut self, path: &Path) -> Result<RawFd, SandboxError> {
        let fd = self
            .fs
            .open_no_symlinks(path)
            .map_err(|source| SandboxError::Io { path: path.to_path_buf(), source })?;
        let number = fd.as_raw_fd();
        self.fds.push((fd, path.to_path_buf()));
        Ok(number)
    }

    /// Opens a descriptor that reads as empty, for one `--ro-bind-data`, and returns
    /// its number.
    pub fn open_empty(&mut self) -> Result<RawFd, SandboxError> {
        let null = PathBuf::from("/dev/null");
        let fd = self
            .fs
            .open_empty()
            .map_err(|source| SandboxError::Io { path: null.clone(), source })?;
        let number = fd.as_raw_fd();
        self.fds.push((fd, null));
        Ok(number)
    }

    /// The numbers of every descriptor, on which the launcher clears `FD_CLOEXEC`.
    pub fn numbers(&self) -> Vec<RawFd> {
        self.fds.iter().map(|(fd, _)| fd.as_raw_fd()).collect()
    }

    /// The descriptors with the path each one was opened on; dropping them closes them
    /// in the launcher once bwrap has started.
    pub fn into_fds(self) -> Vec<(OwnedFd, PathBuf)> {
        self.fds
    }
}

/// The descriptors the launcher made for one launch, besides the bind sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchFds {
    /// The write end of the pipe for `--json-status-fd`.
    pub status: RawFd,
    /// The read end of the pipe that carries the [`InnerPolicy`](crate::InnerPolicy)
    /// to `efr-sbx inner`.
    pub policy: RawFd,
}

impl MountPlan {
    /// The argument list for `--args`: the namespaces, the fixed base, every mount in
    /// order, `--chdir <cwd>` and the inner launcher. Opens one descriptor per bind
    /// into `fds`.
    pub fn bwrap_args(
        &self,
        fds: &mut FdTable<'_>,
        launch: &LaunchFds,
        cwd: &Path,
    ) -> Result<Vec<OsString>, SandboxError> {
        let mut args: Vec<OsString> = Vec::new();
        let mut push = |parts: &[&dyn AsRef<std::ffi::OsStr>]| {
            args.extend(parts.iter().map(|part| part.as_ref().to_os_string()));
        };
        push(&[&"--unshare-user", &"--disable-userns", &"--unshare-pid"]);
        if self.unshare_net {
            push(&[&"--unshare-net"]);
        }
        push(&[&"--unshare-ipc", &"--unshare-uts", &"--unshare-cgroup"]);
        push(&[&"--die-with-parent", &"--cap-drop", &"ALL"]);
        // NOTE: never --new-session (it removes the controlling terminal), never --proc
        // (a new procfs hides the host's processes from ps) and never --dev-bind of
        // /dev (it exposes /dev/uinput and the host's terminals).
        push(&[&"--json-status-fd", &launch.status.to_string()]);
        push(&[&"--ro-bind", &"/", &"/", &"--ro-bind", &"/proc", &"/proc", &"--dev", &"/dev"]);
        for mount in &self.mounts {
            match &mount.op {
                MountOp::Tmpfs { target, perms } => {
                    push(&[&"--perms", &format!("{perms:04o}"), &"--tmpfs", target]);
                }
                // NOTE: not a bind of /dev/null: bwrap mounts every bind but
                // --dev-bind with nodev, so each open of the masked file would fail with
                // EACCES. The data bind is an empty read-only file.
                MountOp::DevNull { target } => {
                    let fd = fds.open_empty()?.to_string();
                    push(&[&"--ro-bind-data", &fd, target]);
                }
                MountOp::Overlay { lower, upper, work, target } => {
                    push(&[&"--overlay-src", lower, &"--overlay", upper, work, target]);
                }
                MountOp::TmpOverlay { lower, target } => {
                    push(&[&"--overlay-src", lower, &"--tmp-overlay", target]);
                }
                MountOp::Bind { source, target, writable } => {
                    let fd = fds.open(source)?.to_string();
                    let flag = if *writable { "--bind-fd" } else { "--ro-bind-fd" };
                    push(&[&flag, &fd, target]);
                }
                MountOp::DevBind { node } => push(&[&"--dev-bind", node, node]),
            }
        }
        push(&[&"--chdir", &cwd]);
        push(&[&"--", &self.inside_launcher, &"inner", &"--policy-fd", &launch.policy.to_string()]);
        Ok(args)
    }
}

/// The argument list as the bytes of the `--args` pipe: each argument followed by a
/// NUL.
pub fn encode_args(args: &[OsString]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let mut out = Vec::new();
    for arg in args {
        out.extend_from_slice(arg.as_bytes());
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests;
