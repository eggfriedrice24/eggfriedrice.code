use std::os::fd::OwnedFd;

use efr_protocol::ApprovalDecision;
use pretty_assertions::assert_eq;
use rustix::fs::{Mode, OFlags};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, tcgetattr};
use tokio::sync::mpsc;

use super::{KeyReader, decision, start_on};

/// A pseudo-terminal pair: the master, which plays the person typing, and the slave,
/// which plays the terminal on stdin.
fn pty() -> (OwnedFd, OwnedFd) {
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

fn canonical(fd: &OwnedFd) -> bool {
    tcgetattr(fd).unwrap().local_modes.contains(LocalModes::ICANON)
}

fn echoes(fd: &OwnedFd) -> bool {
    tcgetattr(fd).unwrap().local_modes.contains(LocalModes::ECHO)
}

/// Waits, without sleeping, until the key thread has switched the terminal.
fn wait_for_key_mode(probe: &OwnedFd) {
    while canonical(probe) {
        std::thread::yield_now();
    }
}

#[test]
fn y_allows_n_denies_and_other_keys_mean_nothing() {
    assert_eq!(decision(b'y'), Some(ApprovalDecision::Allow));
    assert_eq!(decision(b'Y'), Some(ApprovalDecision::Allow));
    assert_eq!(decision(b'n'), Some(ApprovalDecision::Deny));
    assert_eq!(decision(b'N'), Some(ApprovalDecision::Deny));
    for key in [b'\r', b'\n', b' ', b'a', 0x1b, 0x03] {
        assert_eq!(decision(key), None, "{key:#x}");
    }
}

#[tokio::test]
async fn a_reader_yields_keys_until_its_source_ends() {
    let (sender, keys) = mpsc::channel(4);
    let mut reader = KeyReader::from_channel(keys);
    sender.send(b'x').await.unwrap();
    assert_eq!(reader.next().await, Some(b'x'));
    drop(sender);
    assert_eq!(reader.next().await, None);
    reader.stop().await;
}

#[tokio::test]
async fn the_key_thread_reads_one_key_without_enter_or_echo_and_restores_the_terminal() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    assert!(canonical(&probe) && echoes(&probe));

    let mut reader = start_on(slave).unwrap();
    wait_for_key_mode(&probe);
    assert!(!echoes(&probe));
    let signals = tcgetattr(&probe).unwrap().local_modes.contains(LocalModes::ISIG);
    assert!(signals, "Ctrl+C still sends SIGINT, which interrupts the turn");
    rustix::io::write(&master, b"y").unwrap();
    assert_eq!(reader.next().await, Some(b'y'));

    reader.stop().await;
    assert!(canonical(&probe), "the line mode is back");
    assert!(echoes(&probe), "echo is back");
}

#[tokio::test]
async fn keys_typed_before_the_question_are_discarded() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    rustix::io::write(&master, b"y").unwrap();

    let mut reader = start_on(slave).unwrap();
    wait_for_key_mode(&probe);
    rustix::io::write(&master, b"n").unwrap();
    assert_eq!(reader.next().await, Some(b'n'));
    reader.stop().await;
}

#[tokio::test]
async fn a_descriptor_that_is_not_a_terminal_ends_the_keys_at_once() {
    let (read, write) = std::io::pipe().unwrap();
    drop(write);
    let mut reader = start_on(OwnedFd::from(read)).unwrap();
    assert_eq!(reader.next().await, None);
    reader.stop().await;
}
