use std::error::Error as _;
use std::io;
use std::path::PathBuf;

use efr_protocol::{ErrorBody, ErrorCode, ProtocolError, RequestId};
use pretty_assertions::assert_eq;

use super::ClientError;

#[test]
fn a_server_error_shows_the_code_and_the_daemon_message() {
    let error =
        ClientError::Server { body: ErrorBody::new(ErrorCode::NotFound, "no conversation 019a") };
    assert_eq!(
        error.to_string(),
        "the daemon failed the request with not_found: no conversation 019a"
    );
}

#[test]
fn messages_name_what_failed_without_the_source_text() {
    let error = ClientError::Connect {
        socket: PathBuf::from("/run/user/1000/efr/daemon.sock"),
        source: io::Error::from(io::ErrorKind::PermissionDenied),
    };
    assert_eq!(error.to_string(), "could not connect to /run/user/1000/efr/daemon.sock");
    assert!(error.source().is_some());
}

#[test]
fn a_mismatch_names_both_versions() {
    let error = ClientError::ProtocolMismatch { daemon: 2, client: 1 };
    assert_eq!(error.to_string(), "the daemon speaks protocol 2 and this client speaks 1");
}

#[test]
fn an_overflow_names_the_request() {
    let error = ClientError::StreamOverflow { id: RequestId::new(4) };
    assert_eq!(
        error.to_string(),
        "the consumer of request 4 fell behind and the request was cancelled"
    );
}

#[test]
fn protocol_and_io_errors_convert() {
    assert!(matches!(
        ClientError::from(ProtocolError::Truncated { buffered: 1 }),
        ClientError::Protocol { .. }
    ));
    assert!(matches!(
        ClientError::from(io::Error::from(io::ErrorKind::BrokenPipe)),
        ClientError::Io { .. }
    ));
}
