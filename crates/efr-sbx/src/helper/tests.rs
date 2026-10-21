//! The parts of the helpers' shared path that need no sandbox: the names, the ready
//! byte, the place limit and the copy. The real starts and the confinement are in
//! `tests/it/proxy.rs` and `tests/it/forward.rs`.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use super::{Helper, READY, READY_WAIT, Slot, Step, copy_both, read_ready};

#[test]
fn helper_names_fit_the_kernel() {
    for helper in [Helper::Bridge, Helper::Forwarder, Helper::Reach] {
        // The kernel keeps 15 bytes of a process name.
        assert!(helper.name().to_bytes().len() <= 15, "{helper:?}");
        assert!(helper.copy_thread().len() <= 15, "{helper:?}");
    }
    assert_eq!(Helper::Bridge.name().to_bytes(), b"efr-bridge");
    assert_eq!(Helper::Forwarder.name().to_bytes(), b"efr-forward");
    assert_eq!(Helper::Reach.name().to_bytes(), b"efr-reach");
    assert_eq!(Helper::Bridge.to_string(), "network bridge");
    assert_eq!(Helper::Forwarder.to_string(), "port forwarder");
    assert_eq!(Helper::Reach.to_string(), "reach helper");
    let words: Vec<&str> = Step::ALL.iter().map(|step| step.word()).collect();
    assert_eq!(words, ["close", "session", "name", "dumpable", "landlock", "seccomp"]);
    assert_eq!(READY, b'R');
    assert!(READY_WAIT < crate::inner::BRIDGE_WAIT);
}

#[test]
fn ready_byte_or_its_absence() {
    let (read, write) = rustix::pipe::pipe().unwrap();
    std::fs::File::from(write).write_all(b"R").unwrap();
    assert_eq!(read_ready(read, Duration::from_secs(10)), Some(b'R'));
    // The server ended before its byte: the end of the pipe.
    let (read, write) = rustix::pipe::pipe().unwrap();
    drop(write);
    assert_eq!(read_ready(read, Duration::from_secs(10)), None);
    // The server holds the pipe open and sends nothing: the wait ends it.
    let (read, write) = rustix::pipe::pipe().unwrap();
    assert_eq!(read_ready(read, Duration::from_millis(20)), None);
    drop(write);
}

#[test]
fn slots_stop_at_the_limit_and_free_on_drop() {
    let open = Arc::new(AtomicUsize::new(0));
    let first = Slot::take(&open, 2).unwrap();
    let second = Slot::take(&open, 2).unwrap();
    assert!(Slot::take(&open, 2).is_none());
    drop(first);
    let third = Slot::take(&open, 2);
    assert!(third.is_some());
    drop((second, third));
    assert_eq!(open.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn copy_both_carries_both_ways_and_the_half_close() {
    // A TCP pair on an ephemeral loopback port and a Unix pair: the copy joins the
    // test's TCP end to its Unix end.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (served, _) = listener.accept().unwrap();
    let (unix_in, mut unix_out) = UnixStream::pair().unwrap();
    let copier = std::thread::spawn(move || copy_both(Helper::Bridge, served, unix_in));
    let mut client = client;
    client.write_all(b"ping").unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    let mut got = Vec::new();
    unix_out.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"ping", "the end of the TCP side reached the Unix side");
    unix_out.write_all(b"pong").unwrap();
    unix_out.shutdown(Shutdown::Write).unwrap();
    let mut back = Vec::new();
    client.read_to_end(&mut back).unwrap();
    assert_eq!(back, b"pong");
    copier.join().unwrap();
}
