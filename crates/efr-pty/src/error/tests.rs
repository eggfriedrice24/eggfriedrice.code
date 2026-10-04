//! Messages, kinds and the round trip through `io::Error`.

use std::error::Error as _;
use std::io;

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::PtyError;

fn denied() -> io::Error {
    io::Error::from_raw_os_error(libc::EACCES)
}

#[rstest]
#[case(PtyError::OpenMaster { source: denied() }, "could not open a PTY master")]
#[case(
    PtyError::GrantSlave { source: denied() },
    "could not grant access to the slave side of the PTY"
)]
#[case(PtyError::UnlockSlave { source: denied() }, "could not unlock the slave side of the PTY")]
#[case(
    PtyError::SlaveName { source: denied() },
    "could not find the name of the slave side of the PTY"
)]
#[case(
    PtyError::OpenSlave { path: "/dev/pts/7".into(), source: denied() },
    "could not open the PTY slave /dev/pts/7"
)]
#[case(
    PtyError::ConfigureTerminal { source: denied() },
    "could not set up the terminal of the new PTY"
)]
#[case(PtyError::NoRuntime, "the PTY holder needs a tokio runtime to reap its children")]
#[case(PtyError::MissingChildPid, "the new child process has no process id")]
fn the_message_names_the_step_without_the_source_text(
    #[case] error: PtyError,
    #[case] message: &str,
) {
    assert_eq!(error.to_string(), message);
}

#[test]
fn the_system_error_stays_reachable_as_the_source() {
    let error = PtyError::OpenMaster { source: denied() };
    let source = error.source().and_then(|source| source.downcast_ref::<io::Error>());
    assert_eq!(source.and_then(io::Error::raw_os_error), Some(libc::EACCES));
}

#[test]
fn into_io_keeps_the_kind_and_the_step() {
    let wrapped = PtyError::OpenMaster { source: denied() }.into_io();
    assert_eq!(wrapped.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(wrapped.to_string(), "could not open a PTY master");

    let step = wrapped.get_ref().and_then(|inner| inner.downcast_ref::<PtyError>());
    assert!(matches!(step, Some(PtyError::OpenMaster { .. })));
}

#[test]
fn a_step_without_a_system_error_is_kind_other() {
    assert_eq!(PtyError::NoRuntime.into_io().kind(), io::ErrorKind::Other);
    assert_eq!(PtyError::MissingChildPid.into_io().kind(), io::ErrorKind::Other);
}
