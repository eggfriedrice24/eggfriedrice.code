//! The size conversion, and the settings applied to a real PTY pair opened here.

use std::os::fd::OwnedFd;

use efr_holder::Size;
use pretty_assertions::assert_eq;
use rustix::fs::{Mode, OFlags};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{InputModes, Winsize, tcgetattr, tcgetwinsize};

use super::{enable_utf8, foreground_group, set_size, winsize};

/// A master and its slave, both close-on-exec and neither a controlling terminal.
fn pair() -> (OwnedFd, OwnedFd) {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let name = ptsname(&master, Vec::new()).unwrap();
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    (master, slave)
}

#[test]
fn winsize_puts_rows_and_columns_in_place_and_leaves_pixels_unknown() {
    assert_eq!(
        winsize(Size { cols: 120, rows: 40 }),
        Winsize { ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 }
    );
}

#[test]
fn a_size_set_through_the_slave_is_seen_through_the_master() {
    let (master, slave) = pair();
    set_size(&slave, Size { cols: 132, rows: 43 }).unwrap();
    assert_eq!(tcgetwinsize(&master).unwrap(), winsize(Size { cols: 132, rows: 43 }));
}

#[test]
fn a_resize_through_the_master_reaches_the_slave() {
    let (master, slave) = pair();
    set_size(&slave, Size { cols: 80, rows: 24 }).unwrap();
    set_size(&master, Size { cols: 100, rows: 30 }).unwrap();
    assert_eq!(tcgetwinsize(&slave).unwrap(), winsize(Size { cols: 100, rows: 30 }));
}

#[test]
fn enable_utf8_turns_on_iutf8_and_keeps_the_other_input_modes() {
    let (_master, slave) = pair();
    let before = tcgetattr(&slave).unwrap().input_modes;
    enable_utf8(&slave).unwrap();
    let after = tcgetattr(&slave).unwrap().input_modes;
    assert!(after.contains(InputModes::IUTF8));
    assert_eq!(after - InputModes::IUTF8, before - InputModes::IUTF8);
}

#[test]
fn a_terminal_without_a_session_has_no_foreground_group() {
    // Nothing made this PTY a controlling terminal, so it has no session and no
    // foreground group; the holder only asks once its child has made it one.
    let (master, _slave) = pair();
    assert_eq!(foreground_group(&master), Err(rustix::io::Errno::OPNOTSUPP));
}
