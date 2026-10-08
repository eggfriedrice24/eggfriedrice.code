use std::os::fd::OwnedFd;

use efr_protocol::ApprovalDecision;
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use rustix::fs::{Mode, OFlags};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{InputModes, LocalModes, OptionalActions, tcgetattr, tcsetattr};
use tokio::sync::mpsc;

use super::{KEY_QUEUE, Key, KeyReader, Read, Typeahead, decision, start_on};

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

/// Waits until the key thread has switched the terminal.
fn wait_for_key_mode(probe: &OwnedFd) {
    Wait::new("the key mode").until_blocking(|| !canonical(probe)).unwrap();
}

/// A typed byte as the key thread sends it.
fn byte(key: u8) -> Read {
    Read::Key(Key::Byte(key))
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
    sender.send(byte(b'x')).await.unwrap();
    assert_eq!(reader.next().await, Some(Key::Byte(b'x')));
    drop(sender);
    assert_eq!(reader.next().await, None);
    reader.stop().await;
}

#[tokio::test]
async fn the_key_thread_reads_one_key_without_enter_or_echo_and_restores_the_terminal() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    assert!(canonical(&probe) && echoes(&probe));

    let mut reader = start_on(slave, Typeahead::Discard).unwrap();
    wait_for_key_mode(&probe);
    assert!(!echoes(&probe));
    let signals = tcgetattr(&probe).unwrap().local_modes.contains(LocalModes::ISIG);
    assert!(signals, "Ctrl+C still sends SIGINT, which interrupts the turn");
    rustix::io::write(&master, b"y").unwrap();
    assert_eq!(reader.next().await, Some(Key::Byte(b'y')));

    reader.stop().await;
    assert!(canonical(&probe), "the line mode is back");
    assert!(echoes(&probe), "echo is back");
}

#[tokio::test]
async fn keys_typed_before_the_question_are_discarded() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    rustix::io::write(&master, b"y").unwrap();

    let mut reader = start_on(slave, Typeahead::Discard).unwrap();
    wait_for_key_mode(&probe);
    rustix::io::write(&master, b"n").unwrap();
    assert_eq!(reader.next().await, Some(Key::Byte(b'n')));
    reader.stop().await;
}

#[tokio::test]
async fn a_descriptor_that_is_not_a_terminal_ends_the_keys_at_once() {
    let (read, write) = std::io::pipe().unwrap();
    drop(write);
    let mut reader = start_on(OwnedFd::from(read), Typeahead::Discard).unwrap();
    assert_eq!(reader.next().await, None);
    reader.stop().await;
}

#[tokio::test]
async fn stopping_a_reader_whose_queue_is_full_ends_and_restores_the_terminal() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let reader = start_on(slave, Typeahead::Discard).unwrap();
    wait_for_key_mode(&probe);
    // A paste longer than the queue, which nobody reads: the thread ends up blocked
    // on a full queue.
    rustix::io::write(&master, &[b'x'; 2 * KEY_QUEUE]).unwrap();
    full_queue(&reader).await;

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
    let reader = start_on(slave, Typeahead::Discard).unwrap();
    wait_for_key_mode(probe);
    rustix::io::write(master, &[b'x'; 2 * KEY_QUEUE]).unwrap();
    full_queue(&reader).await;
    reader
}

/// Waits until the key thread has filled the queue of `reader`.
async fn full_queue(reader: &KeyReader) {
    Wait::new("a full key queue").until(|| reader.keys.len() >= KEY_QUEUE).await.unwrap();
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
    sender.send(byte(b'a')).await.unwrap();
    sender.send(byte(b'b')).await.unwrap();
    reader.discard_queued();
    sender.send(byte(b'c')).await.unwrap();
    assert_eq!(reader.next().await, Some(Key::Byte(b'c')));
    reader.stop().await;
}

#[tokio::test]
async fn the_rows_reader_keeps_the_keys_typed_before_it_started() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    rustix::io::write(&master, b"ab").unwrap();

    let mut reader = start_on(slave, Typeahead::Keep).unwrap();
    wait_for_key_mode(&probe);
    assert_eq!(reader.next().await, Some(Key::Byte(b'a')));
    assert_eq!(reader.next().await, Some(Key::Byte(b'b')));
    reader.stop().await;
}

#[tokio::test]
async fn enter_and_ctrl_j_stay_two_keys() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let mut reader = start_on(slave, Typeahead::Discard).unwrap();
    wait_for_key_mode(&probe);
    rustix::io::write(&master, b"\r\n").unwrap();
    assert_eq!(reader.next().await, Some(Key::Byte(b'\r')));
    assert_eq!(reader.next().await, Some(Key::Byte(b'\n')));
    reader.stop().await;
    let restored = tcgetattr(&probe).unwrap().input_modes.contains(InputModes::ICRNL);
    assert!(restored, "the map of carriage return to newline is back");
}

#[tokio::test]
async fn an_escape_byte_alone_is_the_esc_key_and_a_sequence_stays_bytes() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let mut reader = start_on(slave, Typeahead::Discard).unwrap();
    wait_for_key_mode(&probe);
    rustix::io::write(&master, b"\x1b").unwrap();
    assert_eq!(reader.next().await, Some(Key::Esc));
    // An arrow key: the terminal writes its bytes in one go.
    rustix::io::write(&master, b"\x1b[A").unwrap();
    let mut keys = Vec::new();
    for _ in 0..3 {
        keys.push(reader.next().await.unwrap());
    }
    assert_eq!(keys, [Key::Byte(0x1b), Key::Byte(b'['), Key::Byte(b'A')]);
    assert_eq!(Key::Esc.byte(), 0x1b, "a line that reads bytes sees the escape byte");
    reader.stop().await;
}

#[tokio::test]
async fn a_flush_throws_away_what_was_typed_and_reads_on() {
    let (master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let mut reader = full_reader(&master, slave, &probe).await;

    reader.flush();
    // The thread throws away what it did not read yet.
    Wait::new("an empty input queue")
        .until_blocking(|| rustix::io::ioctl_fionread(&probe).unwrap() == 0)
        .unwrap();
    rustix::io::write(&master, b"y").unwrap();
    assert_eq!(reader.next().await, Some(Key::Byte(b'y')), "only the key after the flush");
    assert!(!canonical(&probe) && !echoes(&probe), "the mode stays");
    reader.stop().await;
}

#[tokio::test]
async fn a_flush_of_a_reader_without_a_thread_drops_its_queue() {
    let (sender, keys) = mpsc::channel(4);
    let mut reader = KeyReader::from_channel(keys);
    sender.send(byte(b'a')).await.unwrap();
    reader.flush();
    sender.send(byte(b'b')).await.unwrap();
    assert_eq!(reader.next().await, Some(Key::Byte(b'b')));
    reader.stop().await;
}

#[tokio::test]
async fn after_a_stop_the_reader_sets_its_mode_again() {
    let (_master, slave) = pty();
    let probe = rustix::io::dup(&slave).unwrap();
    let reader = start_on(slave, Typeahead::Discard).unwrap();
    wait_for_key_mode(&probe);
    // The shell takes the terminal back while efr is stopped.
    let mut cooked = tcgetattr(&probe).unwrap();
    cooked.local_modes.insert(LocalModes::ICANON | LocalModes::ECHO);
    tcsetattr(&probe, OptionalActions::Now, &cooked).unwrap();

    reader.resumed();
    wait_for_key_mode(&probe);
    assert!(!echoes(&probe));
    reader.stop().await;
}
