//! Terminal settings of a PTY: its window size, its input modes and its foreground
//! process group.
//!
//! On Linux these ioctls work on either side of the pair: the holder sets up a new PTY
//! through the slave before the child starts, and later resizes and queries it through
//! its own copy of the master, because it keeps no slave open once the child runs.

use std::os::fd::AsFd;

use efr_holder::Size;
use rustix::io;
use rustix::process::Pid;
use rustix::termios::{
    InputModes, OptionalActions, Winsize, tcgetattr, tcgetpgrp, tcsetattr, tcsetwinsize,
};

/// The kernel's form of a terminal size. The pixel fields stay 0, which tells programs
/// that the cell size is unknown.
pub(crate) fn winsize(size: Size) -> Winsize {
    Winsize { ws_row: size.rows, ws_col: size.cols, ws_xpixel: 0, ws_ypixel: 0 }
}

/// Sets the window size of the PTY behind `fd`. The kernel sends `SIGWINCH` to the
/// foreground process group when the size changes.
pub(crate) fn set_size(fd: impl AsFd, size: Size) -> io::Result<()> {
    tcsetwinsize(fd, winsize(size))
}

/// Turns on `IUTF8`, so that the line discipline erases a whole UTF-8 character, not
/// one byte of it, when a canonical-mode read sees a backspace. Terminal emulators set
/// it on their PTYs; the kernel's default termios leaves it off.
pub(crate) fn enable_utf8(fd: impl AsFd) -> io::Result<()> {
    let fd = fd.as_fd();
    let mut modes = tcgetattr(fd)?;
    modes.input_modes |= InputModes::IUTF8;
    tcsetattr(fd, OptionalActions::Now, &modes)
}

/// The foreground process group of the PTY behind `master`: where the terminal itself
/// would deliver Ctrl+C. Fails with `OPNOTSUPP` when the terminal has none.
pub(crate) fn foreground_group(master: impl AsFd) -> io::Result<Pid> {
    tcgetpgrp(master)
}

#[cfg(test)]
mod tests;
