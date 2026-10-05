use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::paths::MAX_SOCKET_PATH;
use efr_stdx::time::Clock;

use super::{classify, connect};
use crate::ClientError;
use crate::testing::StoppedClock;

const TIMEOUT: Duration = Duration::from_secs(5);

fn clock() -> Arc<dyn Clock> {
    Arc::new(StoppedClock)
}

#[tokio::test]
async fn a_missing_socket_means_no_daemon_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("daemon.sock");
    let error = connect(&socket, &clock(), TIMEOUT).await.unwrap_err();
    assert!(matches!(error, ClientError::DaemonNotRunning { socket: ref s } if *s == socket));
}

#[tokio::test]
async fn a_stale_socket_means_no_daemon_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("daemon.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    let error = connect(&socket, &clock(), TIMEOUT).await.unwrap_err();
    assert!(matches!(error, ClientError::DaemonNotRunning { .. }));
}

#[tokio::test]
async fn a_socket_path_too_long_for_an_address_is_refused_with_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let long = dir.path().join("x".repeat(MAX_SOCKET_PATH));
    let socket = long.join("daemon.sock");
    let error = connect(&socket, &clock(), TIMEOUT).await.unwrap_err();
    assert!(matches!(error, ClientError::SocketPathTooLong { socket: ref s } if *s == socket));
    let message = error.to_string();
    assert!(message.contains(&format!("more than the {MAX_SOCKET_PATH}")), "{message}");
}

#[tokio::test]
async fn a_listening_socket_connects() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("daemon.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let _stream = connect(&socket, &clock(), TIMEOUT).await.unwrap();
    assert!(listener.accept().await.is_ok());
}

#[test]
fn other_failures_keep_their_cause() {
    let socket = Path::new("/run/user/1000/efr/daemon.sock");
    let cases = [
        (io::ErrorKind::NotFound, "not running"),
        (io::ErrorKind::ConnectionRefused, "not running"),
        (io::ErrorKind::PermissionDenied, "connect"),
        (io::ErrorKind::Other, "connect"),
    ];
    for (kind, expected) in cases {
        let decided = match classify(socket, io::Error::from(kind)) {
            ClientError::DaemonNotRunning { .. } => "not running",
            ClientError::Connect { source, .. } if source.kind() == kind => "connect",
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(decided, expected, "{kind:?}");
    }
}
