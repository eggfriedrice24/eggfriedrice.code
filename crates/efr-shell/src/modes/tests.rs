use std::os::fd::{AsFd as _, OwnedFd};

use rustix::fs::{Mode, OFlags};
use rustix::pty::OpenptFlags;
use rustix::termios::{LocalModes, OptionalActions};

use super::{InputModes, TerminalModes as _, Termios};

/// A fresh PTY pair: the master and the slave, opened in this process.
fn pty() -> (OwnedFd, OwnedFd) {
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let slave =
        rustix::fs::open(name.as_c_str(), OFlags::RDWR | OFlags::NOCTTY, Mode::empty()).unwrap();
    (master, slave)
}

fn set_local_modes(slave: &OwnedFd, change: impl FnOnce(&mut LocalModes)) {
    let mut termios = rustix::termios::tcgetattr(slave).unwrap();
    change(&mut termios.local_modes);
    rustix::termios::tcsetattr(slave, OptionalActions::Now, &termios).unwrap();
}

#[test]
fn a_new_pty_echoes_and_reads_lines() {
    let (master, _slave) = pty();
    assert_eq!(Termios.read(master.as_fd()).unwrap(), InputModes::new(true, true));
}

#[test]
fn the_master_sees_what_the_program_set_on_the_slave() {
    let (master, slave) = pty();
    // What getpass does: echo off, canonical input kept.
    set_local_modes(&slave, |modes| modes.remove(LocalModes::ECHO));
    let modes = Termios.read(master.as_fd()).unwrap();
    assert_eq!(modes, InputModes::new(false, true));
    assert!(modes.hidden());

    // What a line editor does: neither.
    set_local_modes(&slave, |modes| modes.remove(LocalModes::ICANON));
    let modes = Termios.read(master.as_fd()).unwrap();
    assert_eq!(modes, InputModes::new(false, false));
    assert!(!modes.hidden());
}

#[test]
fn a_socket_has_no_modes() {
    let (one, _other) = std::os::unix::net::UnixStream::pair().unwrap();
    let fd = OwnedFd::from(one);
    assert!(Termios.read(fd.as_fd()).is_err());
}

#[test]
fn only_echo_off_with_line_input_is_hidden() {
    assert!(InputModes::new(false, true).hidden());
    assert!(!InputModes::new(true, true).hidden());
    assert!(!InputModes::new(false, false).hidden());
    assert!(!InputModes::new(true, false).hidden());
}

#[test]
fn a_pty_without_a_session_has_no_foreground_group() {
    let (master, _slave) = pty();
    assert!(Termios.foreground(master.as_fd()).is_err());
}
