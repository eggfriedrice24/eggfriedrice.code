use std::os::fd::OwnedFd;

use efr_protocol::ApprovalDecision;
use pretty_assertions::assert_eq;
use rustix::fs::{Mode, OFlags};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, tcgetattr};
use tokio::sync::mpsc;

use super::{KEY_QUEUE, KeyReader, decision, start_on};

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

#[tokio::test]
async fn stopping_a_reader_whose_queue_is_full_ends_and_restores_the_terminal() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let reader = start_on(slave).unwrap();
    wait_for_key_mode(&probe);
    // A paste longer than the queue, which nobody reads: the thread ends up blocked
    // on a full queue.
    rustix::io::write(&master, &[b'x'; 2 * KEY_QUEUE]).unwrap();
    while reader.keys.len() < KEY_QUEUE {
        tokio::task::yield_now().await;
    }

    reader.stop().await;
    assert!(canonical(&probe), "the line mode is back");
    assert!(echoes(&probe), "echo is back");
}

/// What a line read from the terminal on `probe` after `master` ends it with Enter.
fn next_line(master: &OwnedFd, probe: &OwnedFd) -> Vec<u8> {
    rustix::io::write(master, b"\n").unwrap();
    let mut line = [0_u8; 128];
    let read = rustix::io::read(probe, &mut line).unwrap();
    line[..read].to_vec()
}

/// A reader whose thread is blocked on a full queue, with keys still unread behind it.
async fn full_reader(master: &OwnedFd, slave: OwnedFd, probe: &OwnedFd) -> KeyReader {
    let reader = start_on(slave).unwrap();
    wait_for_key_mode(probe);
    rustix::io::write(master, &[b'x'; 2 * KEY_QUEUE]).unwrap();
    while reader.keys.len() < KEY_QUEUE {
        tokio::task::yield_now().await;
    }
    reader
}

#[tokio::test]
async fn stopping_while_discarding_throws_away_the_unread_input() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let reader = full_reader(&master, slave, &probe).await;

    reader.stop_discarding().await;
    assert!(canonical(&probe) && echoes(&probe));
    assert_eq!(next_line(&master, &probe), b"\n", "nothing typed before is left");
}

#[tokio::test]
async fn a_plain_stop_leaves_the_unread_input_for_the_next_reader() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let reader = full_reader(&master, slave, &probe).await;

    reader.stop().await;
    let line = next_line(&master, &probe);
    assert!(line.len() > 1 && line.starts_with(b"x"), "{line:?}");
}

#[tokio::test]
async fn discarding_the_queue_drops_only_the_keys_that_wait_in_it() {
    let (sender, keys) = mpsc::channel(4);
    let mut reader = KeyReader::from_channel(keys);
    sender.send(b'a').await.unwrap();
    sender.send(b'b').await.unwrap();
    reader.discard_queued();
    sender.send(b'c').await.unwrap();
    assert_eq!(reader.next().await, Some(b'c'));
    reader.stop().await;
}
