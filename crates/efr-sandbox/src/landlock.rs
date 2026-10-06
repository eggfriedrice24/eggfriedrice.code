//! [`LandlockPolicy`]: the Landlock rule set of one contained call, as data.
//!
//! `efr-sbx inner` turns it into a ruleset with the `landlock` crate at
//! `CompatLevel::HardRequirement`: a right the kernel lacks fails the call, it is
//! never dropped. Writes outside the roots then fail twice, with `EACCES` from
//! Landlock and `EROFS` from the read-only bind.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::plan::MountPlan;

/// The lowest Landlock ABI that `auto` accepts: 9 brings `RESOLVE_UNIX` (Linux 7.1).
pub const MIN_LANDLOCK_ABI: u32 = 9;

/// The erratum that must be fixed: rights widened through rename or link on
/// disconnected directories under bind mounts (erratum 3, bit 2 of
/// `LANDLOCK_CREATE_RULESET_ERRATA`).
pub const ERRATUM_DISCONNECTED_DIRS: u32 = 1 << 2;

/// A file system right of Landlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FsAccess {
    /// Run a file.
    Execute,
    /// Write a file.
    WriteFile,
    /// Read a file.
    ReadFile,
    /// List a directory.
    ReadDir,
    /// Remove a directory.
    RemoveDir,
    /// Remove a file.
    RemoveFile,
    /// Make a character device.
    MakeChar,
    /// Make a directory.
    MakeDir,
    /// Make a regular file.
    MakeReg,
    /// Make a socket.
    MakeSock,
    /// Make a fifo.
    MakeFifo,
    /// Make a block device.
    MakeBlock,
    /// Make a symbolic link.
    MakeSym,
    /// Link or rename a file into another directory.
    Refer,
    /// Truncate a file.
    Truncate,
    /// `ioctl` on a device.
    IoctlDev,
    /// Connect to a Unix socket by path (ABI 9).
    ResolveUnix,
}

impl FsAccess {
    /// Every right up to ABI 9: the handled rights of the rule set.
    pub const HANDLED: [FsAccess; 17] = [
        FsAccess::Execute,
        FsAccess::WriteFile,
        FsAccess::ReadFile,
        FsAccess::ReadDir,
        FsAccess::RemoveDir,
        FsAccess::RemoveFile,
        FsAccess::MakeChar,
        FsAccess::MakeDir,
        FsAccess::MakeReg,
        FsAccess::MakeSock,
        FsAccess::MakeFifo,
        FsAccess::MakeBlock,
        FsAccess::MakeSym,
        FsAccess::Refer,
        FsAccess::Truncate,
        FsAccess::IoctlDev,
        FsAccess::ResolveUnix,
    ];

    /// The rule on `/`: read and run everything.
    pub const READ_EXEC: [FsAccess; 3] = [FsAccess::Execute, FsAccess::ReadFile, FsAccess::ReadDir];

    /// The rule on a writable directory: every handled right except devices,
    /// `ioctl` on devices and `RESOLVE_UNIX`.
    pub const WRITE_DIR: [FsAccess; 13] = [
        FsAccess::Execute,
        FsAccess::WriteFile,
        FsAccess::ReadFile,
        FsAccess::ReadDir,
        FsAccess::RemoveDir,
        FsAccess::RemoveFile,
        FsAccess::MakeDir,
        FsAccess::MakeReg,
        FsAccess::MakeSock,
        FsAccess::MakeFifo,
        FsAccess::MakeSym,
        FsAccess::Refer,
        FsAccess::Truncate,
    ];

    /// The rule on one writable file of a grant: the rights a file can hold.
    pub const WRITE_FILE: [FsAccess; 4] =
        [FsAccess::Execute, FsAccess::WriteFile, FsAccess::ReadFile, FsAccess::Truncate];

    /// The rule on `/dev/null`, `/dev/zero` and `/dev/full`.
    pub const DEV_RW: [FsAccess; 2] = [FsAccess::ReadFile, FsAccess::WriteFile];

    /// The rule on a terminal and on a granted device.
    pub const DEVICE: [FsAccess; 3] = [FsAccess::ReadFile, FsAccess::WriteFile, FsAccess::IoctlDev];

    /// The rule on a granted socket.
    pub const SOCKET: [FsAccess; 1] = [FsAccess::ResolveUnix];
}

/// A Landlock scope: what a sandboxed process cannot reach outside its domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LandlockScope {
    /// Abstract Unix sockets of processes outside the domain.
    AbstractUnixSocket,
    /// Signals to processes outside the domain.
    Signal,
}

/// One path rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandlockRule {
    /// The path, as the sandbox sees it.
    pub path: PathBuf,
    /// The rights below it.
    pub access: Vec<FsAccess>,
}

/// The Landlock rule set of one contained call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LandlockPolicy {
    /// The lowest ABI that the kernel must have, [`MIN_LANDLOCK_ABI`].
    pub min_abi: u32,
    /// The errata bits that the kernel must report fixed.
    pub required_errata: u32,
    /// Paths with [`FsAccess::READ_EXEC`]: `/`.
    pub read_exec: Vec<PathBuf>,
    /// Directories with [`FsAccess::WRITE_DIR`].
    pub write: Vec<PathBuf>,
    /// Files of write grants with [`FsAccess::WRITE_FILE`].
    pub write_files: Vec<PathBuf>,
    /// Paths with [`FsAccess::DEV_RW`].
    pub dev_rw: Vec<PathBuf>,
    /// The inherited descriptors that get [`FsAccess::DEVICE`] when they are a
    /// terminal, so `echo > /dev/stderr` works.
    pub tty_fds: Vec<i32>,
    /// Sockets with [`FsAccess::SOCKET`]: none in phase 1 without a grant.
    pub resolve_unix: Vec<PathBuf>,
    /// Paths with [`FsAccess::DEVICE`]: the terminal nodes and the granted devices.
    pub devices: Vec<PathBuf>,
    /// The scopes.
    pub scopes: Vec<LandlockScope>,
}

impl LandlockPolicy {
    /// Every path rule with its rights, in a fixed order. The descriptor rules of
    /// `tty_fds` are not in it.
    pub fn rules(&self) -> Vec<LandlockRule> {
        let mut rules = Vec::new();
        let mut add = |paths: &[PathBuf], access: &[FsAccess]| {
            for path in paths {
                rules.push(LandlockRule { path: path.clone(), access: access.to_vec() });
            }
        };
        add(&self.read_exec, &FsAccess::READ_EXEC);
        add(&self.write, &FsAccess::WRITE_DIR);
        add(&self.write_files, &FsAccess::WRITE_FILE);
        add(&self.dev_rw, &FsAccess::DEV_RW);
        add(&self.devices, &FsAccess::DEVICE);
        add(&self.resolve_unix, &FsAccess::SOCKET);
        rules
    }
}

impl MountPlan {
    /// The Landlock rule set that matches this plan.
    pub fn landlock(&self) -> LandlockPolicy {
        let mut devices: Vec<PathBuf> =
            ["/dev/tty", "/dev/console", "/dev/ptmx", "/dev/pts"].map(PathBuf::from).to_vec();
        devices.extend(self.devices.iter().cloned());
        LandlockPolicy {
            min_abi: MIN_LANDLOCK_ABI,
            required_errata: ERRATUM_DISCONNECTED_DIRS,
            read_exec: vec![PathBuf::from("/")],
            write: self.write_dirs.clone(),
            write_files: self.write_files.clone(),
            dev_rw: ["/dev/null", "/dev/zero", "/dev/full"].map(PathBuf::from).to_vec(),
            tty_fds: vec![0, 1, 2],
            resolve_unix: self.resolve_unix.clone(),
            devices,
            scopes: vec![LandlockScope::AbstractUnixSocket, LandlockScope::Signal],
        }
    }
}

#[cfg(test)]
mod tests;
