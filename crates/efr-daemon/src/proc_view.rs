//! What routing reads about processes and terminals: whether a shell still runs, and
//! which terminal a socket peer runs on.
//!
//! Production reads `/proc` and the device numbers of `/dev/pts/*`
//! ([`SystemProcView`]); a test daemon injects a fake through
//! [`Deps::with_proc_view`](crate::Deps::with_proc_view), so no routing test depends
//! on a real process. No config key reaches it.

use std::fmt::Debug;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

/// The process facts that routing reads.
pub trait ProcView: Send + Sync + Debug {
    /// True while the process `pid` runs. A zombie (state `Z`), a process that has
    /// exited and waits for its parent, counts as gone. A pid that the kernel gave to
    /// another process counts as running, which only ever stops a pick-up.
    fn alive(&self, pid: u32) -> bool;

    /// The device number of the controlling terminal of the process `pid` (field 7,
    /// `tty_nr`, of `/proc/<pid>/stat`), in the form [`ProcView::device_of`] gives.
    /// `None` when it has none or it cannot be read.
    fn tty_of(&self, pid: u32) -> Option<u64>;

    /// The device number of the terminal `tty`, such as `/dev/pts/3` (its `st_rdev`).
    /// `None` when it is no device or it cannot be read.
    fn device_of(&self, tty: &str) -> Option<u64>;
}

/// The process facts of this machine, from `/proc` and `/dev`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemProcView;

impl ProcView for SystemProcView {
    fn alive(&self, pid: u32) -> bool {
        match std::fs::read_to_string(stat_path(pid)) {
            Ok(text) => parse_stat(&text).is_none_or(|stat| stat.state != 'Z'),
            // NOTE: a process whose stat cannot be read but whose dir exists counts as
            // running: the safe side for a pick-up.
            Err(_) => Path::new("/proc").join(pid.to_string()).exists(),
        }
    }

    fn tty_of(&self, pid: u32) -> Option<u64> {
        let text = std::fs::read_to_string(stat_path(pid)).ok()?;
        let stat = parse_stat(&text)?;
        (stat.tty_nr != 0).then(|| tty_nr_device(stat.tty_nr))
    }

    fn device_of(&self, tty: &str) -> Option<u64> {
        let meta = std::fs::metadata(tty).ok()?;
        let rdev = meta.rdev();
        (rdev != 0).then_some(rdev)
    }
}

fn stat_path(pid: u32) -> std::path::PathBuf {
    Path::new("/proc").join(pid.to_string()).join("stat")
}

/// The fields of `/proc/<pid>/stat` that routing reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stat {
    /// Field 3: `R`, `S`, `Z` and the others.
    pub(crate) state: char,
    /// Field 7: the controlling terminal, in the kernel's packed form.
    pub(crate) tty_nr: u32,
}

/// Reads fields 3 and 7 of a `/proc/<pid>/stat` line. The command name in field 2 is
/// in parentheses and may hold spaces and parentheses, so the fields after it start
/// after its last `)`.
pub(crate) fn parse_stat(text: &str) -> Option<Stat> {
    let rest = &text[text.rfind(')')? + 1..];
    let mut fields = rest.split_ascii_whitespace();
    let state = fields.next()?.chars().next()?;
    // ppid, pgrp, session, then tty_nr.
    let tty_nr = fields.nth(3)?.parse::<i64>().ok()?;
    Some(Stat { state, tty_nr: u32::try_from(tty_nr).ok()? })
}

/// The device number of a packed `tty_nr`, in glibc's `makedev` form, which `st_rdev`
/// also has. The kernel packs the minor number into bits 0-7 and 20-31 and the major
/// number into bits 8-19 (`new_encode_dev`).
pub(crate) fn tty_nr_device(tty_nr: u32) -> u64 {
    let major = u64::from((tty_nr >> 8) & 0xfff);
    let minor = u64::from((tty_nr & 0xff) | ((tty_nr >> 12) & 0xfff00));
    makedev(major, minor)
}

/// glibc's `makedev`.
pub(crate) fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xffff_f000) << 32)
        | ((major & 0x0000_0fff) << 8)
        | ((minor & 0xffff_ff00) << 12)
        | (minor & 0x0000_00ff)
}

#[cfg(test)]
mod tests;
