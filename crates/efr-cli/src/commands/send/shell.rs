//! The shell of `efr send` typed by hand (DV55, check first VR1-9): its parent process,
//! when that runs on the same terminal. efrd keeps a terminal's slot for the shell that
//! took it, so a slot taken by hand is not held forever once that shell ends.
//!
//! The parent's terminal is field 7 (`tty_nr`) of `/proc/<ppid>/stat`, compared with
//! the device number of the terminal on stdin. When either cannot be read, efr names
//! no shell, as before.

use std::os::unix::fs::MetadataExt as _;

/// The pid of efr's parent when it runs on the terminal `tty`, such as `/dev/pts/3`.
pub(crate) fn parent_on(tty: &str) -> Option<u32> {
    let parent = rustix::process::getppid()?;
    let pid = u32::try_from(parent.as_raw_pid()).ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let device = std::fs::metadata(tty).ok()?.rdev();
    on_device(&stat, device).then_some(pid)
}

/// True when the process of the `/proc/<pid>/stat` line `stat` runs on the terminal
/// with the device number `device` (`st_rdev`).
pub(crate) fn on_device(stat: &str, device: u64) -> bool {
    tty_nr(stat).is_some_and(|tty_nr| tty_nr != 0 && tty_device(tty_nr) == device)
}

/// Field 7 of a stat line. The command name in field 2 may hold spaces and
/// parentheses, so the fields after it start after its last `)`.
fn tty_nr(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // state, ppid, pgrp, session, then tty_nr.
    let value = rest.split_ascii_whitespace().nth(4)?.parse::<i64>().ok()?;
    u32::try_from(value).ok()
}

/// The device number of a packed `tty_nr`, in glibc's `makedev` form, as `st_rdev`
/// has it: the kernel packs the minor number into bits 0-7 and 20-31 and the major
/// number into bits 8-19.
pub(crate) fn tty_device(tty_nr: u32) -> u64 {
    let major = u64::from((tty_nr >> 8) & 0xfff);
    let minor = u64::from((tty_nr & 0xff) | ((tty_nr >> 12) & 0xfff00));
    ((major & 0xffff_f000) << 32)
        | ((major & 0x0000_0fff) << 8)
        | ((minor & 0xffff_ff00) << 12)
        | (minor & 0x0000_00ff)
}

#[cfg(test)]
mod tests;
