use std::io::{self, Write};

use pretty_assertions::assert_eq;

use super::{Output, set_restore, take_restore};
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
