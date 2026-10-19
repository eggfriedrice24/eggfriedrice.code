//! The programs that efrd itself runs: `crontab` and `loginctl` for the state copies of
//! cron and linger (efr's auto spec 10.8, plan PB20), and `xdg-dbus-proxy` for the
//! filtered bus of the `auto` sandbox (phase E, DE7).
//!
//! efrd finds them once at start, on its own `PATH`, never on the hidden shell's,
//! whose entries can be dirs that the sandbox wrote. A program counts only in a dir
//! that is absolute, owned by root, writable by root only, and in no place that a call
//! can write: not below the home directory, efr's roots, `/tmp`, `/var/tmp` or
//! `/dev/shm`. The program file itself must be owned by root and writable by root only.
//! efrd keeps the device and inode of the file and checks them again before each run,
//! so a file that someone puts in its place never runs. With no such program, the state
//! copies are off on that machine and the forms that need them stay with the user.
//!
//! Each run has fixed arguments, a clean environment and no word of the model.

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{Mode, OFlags};

/// The dirs where a call can write, besides the home directory and efr's roots.
const WRITABLE_DIRS: [&str; 3] = ["/tmp", "/var/tmp", "/dev/shm"];

/// A program that efrd runs itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Program {
    /// `crontab`, for the user's crontab.
    Crontab,
    /// `loginctl`, for the linger setting.
    Loginctl,
    /// `xdg-dbus-proxy`, the filter of a call's bus (phase E).
    XdgDbusProxy,
}

impl Program {
    const ALL: [Program; 3] = [Program::Crontab, Program::Loginctl, Program::XdgDbusProxy];

    /// The file name on the `PATH`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Program::Crontab => "crontab",
            Program::Loginctl => "loginctl",
            Program::XdgDbusProxy => "xdg-dbus-proxy",
        }
    }
}

/// One program that efrd found, with the device and inode that it had then.
///
/// `pin` keeps the file open (`O_PATH`) while efrd runs. So its inode stays in use, and
/// a file that someone puts in its place always gets another inode number: a file
/// system that gives a freed inode number to the next new file (tmpfs does) cannot
/// fool the check before a run.
#[derive(Debug, Clone)]
struct Found {
    program: Program,
    path: PathBuf,
    dev: u64,
    ino: u64,
    pin: Arc<OwnedFd>,
}

impl PartialEq for Found {
    fn eq(&self, other: &Self) -> bool {
        (self.program, &self.path, self.dev, self.ino)
            == (other.program, &other.path, other.dev, other.ino)
    }
}

impl Eq for Found {}

/// The programs that efrd may run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrustedPrograms {
    found: Vec<Found>,
}

impl TrustedPrograms {
    /// No program: the state copies are off.
    pub(crate) fn none() -> Self {
        TrustedPrograms::default()
    }

    /// The programs on `path` (efrd's own `PATH`) that pass the rule of this module.
    /// `refused` are the dirs where a call can write: the home directory and efr's
    /// roots. It blocks.
    pub(crate) fn resolve(path: &OsStr, refused: &[PathBuf]) -> Self {
        Self::find(path, refused, true)
    }

    /// The programs of the dir `dir` for a test that puts fake programs there: without
    /// the owner rule, and without the system temp dirs, where tests make their dirs;
    /// the rest of the rule holds. It blocks.
    pub(crate) fn in_test_dir(dir: &Path, refused: &[PathBuf]) -> Self {
        Self::find(dir.as_os_str(), refused, false)
    }

    fn find(path: &OsStr, refused: &[PathBuf], root_only: bool) -> Self {
        let mut refused = refused.to_vec();
        if root_only {
            refused.extend(WRITABLE_DIRS.iter().map(PathBuf::from));
        }
        let mut found = Vec::new();
        for program in Program::ALL {
            let entries = std::env::split_paths(path);
            let hit = entries
                .filter(|dir| trusted_dir(dir, &refused, root_only))
                .map(|dir| dir.join(program.name()))
                .find_map(|candidate| trusted_file(&candidate, root_only));
            if let Some((path, dev, ino, pin)) = hit {
                found.push(Found { program, path, dev, ino, pin: Arc::new(pin) });
            }
        }
        TrustedPrograms { found }
    }

    /// True when efrd found `program`.
    pub(crate) fn has(&self, program: Program) -> bool {
        self.found.iter().any(|found| found.program == program)
    }

    /// The path of `program` after the check that [`command`](Self::command) makes:
    /// `None` when efrd found no such program or the file is no longer the one that it
    /// found. It blocks for one stat.
    pub(crate) fn checked_path(&self, program: Program) -> Option<PathBuf> {
        let found = self.found.iter().find(|found| found.program == program)?;
        let meta = std::fs::symlink_metadata(&found.path).ok()?;
        let pinned = rustix::fs::fstat(&*found.pin).ok()?;
        let same = (meta.dev(), meta.ino()) == (found.dev, found.ino)
            && (pinned.st_dev, pinned.st_ino) == (found.dev, found.ino);
        if !same || !meta.is_file() {
            tracing::warn!(program = program.name(), path = %found.path.display(), "a trusted program changed since efrd started; it does not run");
            return None;
        }
        Some(found.path.clone())
    }

    /// A command for `program` with a clean environment, or `None` when efrd found no
    /// such program or the file is no longer the one that it found
    /// ([`checked_path`](Self::checked_path)). The caller adds the fixed arguments.
    pub(crate) fn command(&self, program: Program) -> Option<tokio::process::Command> {
        let path = self.checked_path(program)?;
        let mut command = efr_stdx::process::command(&path, "/");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        Some(command)
    }
}

/// True when a program in `dir` may run: an absolute dir in none of `refused`,
/// writable only by its owner, root when `root_only`.
fn trusted_dir(dir: &Path, refused: &[PathBuf], root_only: bool) -> bool {
    if !dir.is_absolute() || dir.components().any(|part| part.as_os_str() == "..") {
        return false;
    }
    if refused.iter().any(|root| dir.starts_with(root)) {
        return false;
    }
    let Ok(real) = std::fs::canonicalize(dir) else { return false };
    if refused.iter().any(|root| real.starts_with(root)) {
        return false;
    }
    let Ok(meta) = std::fs::metadata(&real) else { return false };
    meta.is_dir() && owned(&meta, root_only) && meta.mode() & 0o022 == 0
}

/// The path, device and inode of `candidate` when it is a program that may run: a
/// regular file, not a link, writable only by its owner, root when `root_only`. With
/// them comes the open file that keeps the inode in use.
fn trusted_file(candidate: &Path, root_only: bool) -> Option<(PathBuf, u64, u64, OwnedFd)> {
    let meta = std::fs::symlink_metadata(candidate).ok()?;
    let runs = meta.mode() & 0o111 != 0;
    if !(meta.is_file() && runs && owned(&meta, root_only) && meta.mode() & 0o022 == 0) {
        return None;
    }
    let flags = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let pin = rustix::fs::open(candidate, flags, Mode::empty()).ok()?;
    let held = rustix::fs::fstat(&pin).ok()?;
    (held.st_dev == meta.dev() && held.st_ino == meta.ino())
        .then(|| (candidate.to_path_buf(), meta.dev(), meta.ino(), pin))
}

/// True when root owns `meta`, or with `root_only` false also efrd's own user.
fn owned(meta: &std::fs::Metadata, root_only: bool) -> bool {
    meta.uid() == 0 || (!root_only && meta.uid() == rustix::process::getuid().as_raw())
}

#[cfg(test)]
mod tests;
