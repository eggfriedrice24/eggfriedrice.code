use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt as _;
use tokio::net::UnixStream;

use super::{UnixListener, authorize};
use crate::{PeerCred, TransportError};

fn own_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn bind_creates_a_0600_socket_in_a_new_0700_directory() {
    let dir = tempfile::tempdir().unwrap();
    let socket_dir = dir.path().join("efr");
    let path = socket_dir.join("daemon.sock");
    let listener = UnixListener::bind(&path).await.unwrap();
    assert_eq!(listener.path(), path);
    assert_eq!(listener.allowed_uid(), own_uid());
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&socket_dir), 0o700);
    let names: Vec<_> =
        fs::read_dir(&socket_dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, vec!["daemon.sock"], "no temporary socket is left behind");
}

#[tokio::test]
async fn bind_leaves_an_existing_directory_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o750)).unwrap();
    let _listener = UnixListener::bind(dir.path().join("daemon.sock")).await.unwrap();
    assert_eq!(mode(dir.path()), 0o750);
}

#[tokio::test]
async fn accept_reports_the_kernel_credentials_and_numbers_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&path).await.unwrap();
    let _first = UnixStream::connect(&path).await.unwrap();
    let _second = UnixStream::connect(&path).await.unwrap();
    let first = listener.accept().await.unwrap();
    let second = listener.accept().await.unwrap();
    assert_eq!(first.peer, PeerCred::new(own_uid(), Some(std::process::id())));
    assert_eq!((first.conn_id.get(), second.conn_id.get()), (1, 2));
}

#[tokio::test]
async fn a_peer_of_another_uid_is_rejected_and_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let other = own_uid().wrapping_add(1);
    let listener = UnixListener::bind_allowing(path.clone(), other).await.unwrap();
    let mut client = UnixStream::connect(&path).await.unwrap();
    let error = listener.accept().await.unwrap_err();
    assert!(
        matches!(
            error,
            TransportError::PeerRejected { uid, pid, allowed }
                if uid == own_uid() && pid == Some(std::process::id()) && allowed == other
        ),
        "{error:?}"
    );
    let mut buffer = [0; 1];
    assert_eq!(client.read(&mut buffer).await.unwrap(), 0, "the rejected peer sees EOF");
}

#[test]
fn authorize_lets_in_only_the_allowed_uid() {
    assert!(authorize(PeerCred::new(1000, Some(1)), 1000).is_ok());
    assert!(matches!(
        authorize(PeerCred::new(0, None), 1000),
        Err(TransportError::PeerRejected { uid: 0, pid: None, allowed: 1000 })
    ));
    assert!(matches!(
        authorize(PeerCred::new(1001, Some(9)), 1000),
        Err(TransportError::PeerRejected { uid: 1001, pid: Some(9), allowed: 1000 })
    ));
}

#[tokio::test]
async fn a_stale_socket_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists(), "a dropped std listener leaves its file behind");
    let listener = UnixListener::bind(&path).await.unwrap();
    let _client = UnixStream::connect(&path).await.unwrap();
    assert!(listener.accept().await.is_ok());
    assert_eq!(mode(&path), 0o600);
}

#[tokio::test]
async fn a_socket_that_answers_is_never_taken_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let _running = UnixListener::bind(&path).await.unwrap();
    let error = UnixListener::bind(&path).await.unwrap_err();
    assert!(matches!(error, TransportError::AddressInUse { path: ref p } if *p == path));
    assert!(path.exists());
}

#[tokio::test]
async fn something_that_is_not_a_socket_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    fs::write(&path, b"precious").unwrap();
    let error = UnixListener::bind(&path).await.unwrap_err();
    assert!(matches!(error, TransportError::NotASocket { .. }));
    assert_eq!(fs::read(&path).unwrap(), b"precious");
}

#[tokio::test]
async fn dropping_the_listener_removes_its_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    drop(UnixListener::bind(&path).await.unwrap());
    assert!(!path.exists());
}

#[tokio::test]
async fn dropping_a_replaced_listener_leaves_its_successor_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let first = UnixListener::bind(&path).await.unwrap();
    fs::remove_file(&path).unwrap();
    let second = UnixListener::bind(&path).await.unwrap();
    drop(first);
    assert!(path.exists());
    let _client = UnixStream::connect(&path).await.unwrap();
    assert!(second.accept().await.is_ok());
}
