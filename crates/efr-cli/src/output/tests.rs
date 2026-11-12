use std::io::{self, LineWriter, Write};
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixDatagram;

use pretty_assertions::assert_eq;

use super::{Output, set_restore, take_restore, unbuffered};
use crate::error::CliError;
use crate::testing::capture;

/// A writer whose reader went away.
struct ClosedPipe;

impl Write for ClosedPipe {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn stdout_and_stderr_stay_apart() {
    let (mut out, captured) = capture();
    out.out("reply\n").unwrap();
    out.err("note\n");
    out.out("more\n").unwrap();
    assert_eq!(captured.stdout(), "reply\nmore\n");
    assert_eq!(captured.stderr(), "note\n");
}

#[test]
fn a_closed_stdout_is_an_error_not_a_panic() {
    let mut out = Output::from_writers(Box::new(ClosedPipe), Box::new(io::sink()));
    let error = out.out("reply\n").unwrap_err();
    assert!(
        matches!(error, CliError::Output { ref source } if source.kind() == io::ErrorKind::BrokenPipe)
    );
    assert!(error.is_silent());
}

#[test]
fn a_closed_stderr_is_ignored() {
    let mut out = Output::from_writers(Box::new(io::sink()), Box::new(ClosedPipe));
    out.err("nobody hears this\n");
    out.out("").unwrap();
}

#[test]
fn empty_text_writes_nothing() {
    let mut out = Output::from_writers(Box::new(ClosedPipe), Box::new(ClosedPipe));
    out.out("").unwrap();
    out.err("");
}

#[test]
fn the_restore_bytes_are_the_newest_and_are_taken_once() {
    set_restore("\x1b[?25h");
    set_restore("\x1b[?25h\x1b]9;4;0\x1b\\");
    assert_eq!(take_restore(), "\x1b[?25h\x1b]9;4;0\x1b\\");
    assert_eq!(take_restore(), "");
}

/// The write calls that reach `socket`: a datagram socket keeps each one apart.
fn calls(socket: &UnixDatagram) -> Vec<String> {
    socket.set_nonblocking(true).unwrap();
    let mut calls = Vec::new();
    let mut buf = [0_u8; 4096];
    while let Ok(len) = socket.recv(&mut buf) {
        calls.push(String::from_utf8_lossy(&buf[..len]).into_owned());
    }
    calls
}

#[test]
fn a_frame_with_newlines_is_one_write_call() {
    let frame = "\x1b[?2026h\r\x1b[2Aone\ntwo\n\x1b[1A\r\x1b[5C\x1b[?2026l";
    let (writer, reader) = UnixDatagram::pair().unwrap();
    let mut out = Output::from_writers(unbuffered(writer.as_fd()).unwrap(), Box::new(io::sink()));
    out.out(frame).unwrap();
    out.out("next\n").unwrap();
    assert_eq!(calls(&reader), [frame, "next\n"]);
    // The line buffer of `io::stdout` makes two calls of the same frame.
    let (writer, reader) = UnixDatagram::pair().unwrap();
    let mut buffered =
        LineWriter::new(std::fs::File::from(writer.as_fd().try_clone_to_owned().unwrap()));
    buffered.write_all(frame.as_bytes()).unwrap();
    buffered.flush().unwrap();
    assert_eq!(calls(&reader).len(), 2);
}
