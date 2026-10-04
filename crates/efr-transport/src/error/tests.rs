use std::error::Error as _;
use std::io;
use std::path::PathBuf;

use efr_protocol::{ProtocolError, Seq};
use pretty_assertions::assert_eq;

use super::TransportError;

#[test]
fn messages_name_what_failed_without_the_source_text() {
    let error = TransportError::Bind {
        path: PathBuf::from("/run/user/1000/efr/daemon.sock"),
        source: io::Error::other("secret detail"),
    };
    assert_eq!(error.to_string(), "could not bind a Unix socket at /run/user/1000/efr/daemon.sock");
    assert_eq!(error.source().unwrap().to_string(), "secret detail");
}

#[test]
fn a_rejected_peer_names_both_uids() {
    let error = TransportError::PeerRejected { uid: 1001, pid: Some(42), allowed: 1000 };
    assert_eq!(error.to_string(), "a process of uid 1001 connected, but only uid 1000 may");
}

#[test]
fn an_overflow_names_the_last_seq() {
    let error = TransportError::Overflow { last_seq: Seq::new(17) };
    assert_eq!(error.to_string(), "the subscriber fell behind after seq 17");
}

#[test]
fn protocol_errors_convert_and_keep_their_source() {
    let error = TransportError::from(ProtocolError::Truncated { buffered: 3 });
    assert!(matches!(error, TransportError::Protocol { .. }));
    assert_eq!(
        error.source().unwrap().to_string(),
        "the stream ended inside a frame, after 3 bytes of it"
    );
}

#[test]
fn io_errors_convert_for_the_codec() {
    let error = TransportError::from(io::Error::from(io::ErrorKind::BrokenPipe));
    assert!(matches!(error, TransportError::Io { .. }));
}
