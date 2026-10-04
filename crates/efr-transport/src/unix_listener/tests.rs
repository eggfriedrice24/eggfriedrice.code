use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use efr_protocol::{RequestId, ServerFrame};
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::AsyncReadExt as _;
use tokio::net::UnixStream;
use tokio::sync::oneshot;

use super::{UnixListener, authorize, stage, staging_dir};
use crate::testing::{FakeDispatcher, StoppedClock, TestClient, internal, list_frame};
use crate::{PeerCred, Request, TransportError};

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
async fn the_socket_is_restricted_inside_a_private_directory_before_it_gets_its_name() {
    let dir = tempfile::tempdir().unwrap();
    // A parent that others may enter, as a shared runtime directory might be.
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.path().join("daemon.sock");
    let staging = staging_dir(&path);
    assert_eq!(staging.parent(), Some(dir.path()), "the rename stays in one directory");
    let (_listener, temp) = stage(&staging, &path).unwrap();
    assert_eq!(temp.parent(), Some(staging.as_path()));
    assert_eq!(mode(&staging), 0o700, "only the owner can reach the staged socket");
    assert_eq!(mode(&temp), 0o600);
    assert!(!path.exists(), "the socket has no public name yet");
}

#[tokio::test]
async fn bind_moves_the_socket_out_of_the_staging_directory_and_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.path().join("daemon.sock");
    let _listener = UnixListener::bind(&path).await.unwrap();
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(dir.path()), 0o755, "an existing parent keeps its mode");
    assert!(!staging_dir(&path).exists());
}

#[tokio::test]
async fn a_staging_directory_left_by_a_crash_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let staging = staging_dir(&path);
    fs::create_dir(&staging).unwrap();
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o777)).unwrap();
    drop(std::os::unix::net::UnixListener::bind(staging.join("daemon.sock")).unwrap());
    let listener = UnixListener::bind(&path).await.unwrap();
    assert!(!staging.exists());
    let _client = UnixStream::connect(&path).await.unwrap();
    assert!(listener.accept().await.is_ok());
}

#[tokio::test]
async fn a_staging_directory_with_foreign_content_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let staging = staging_dir(&path);
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("notes.txt"), b"precious").unwrap();
    fs::write(staging.join("daemon.sock"), b"not a socket").unwrap();
    let error = UnixListener::bind(&path).await.unwrap_err();
    assert!(matches!(error, TransportError::Inspect { path: ref p, .. } if *p == staging));
    assert_eq!(fs::read(staging.join("notes.txt")).unwrap(), b"precious");
    assert_eq!(fs::read(staging.join("daemon.sock")).unwrap(), b"not a socket");
    assert!(!path.exists());
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

#[tokio::test]
async fn serve_answers_over_the_socket_and_cleans_up_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&path).await.unwrap();
    let dispatcher = FakeDispatcher::new(|request: Request| async move {
        request.responder.item(&json!({ "served": true })).await.map_err(internal)
    });
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(listener.serve(dispatcher, Arc::new(StoppedClock), async {
        let _ = stopped.await;
    }));

    let mut client = TestClient::new(UnixStream::connect(&path).await.unwrap());
    client.hello().await;
    client.send(&list_frame(2)).await;
    assert_eq!(
        client.recv().await,
        Some(ServerFrame::Item { id: RequestId::new(2), item: json!({ "served": true }) })
    );
    assert_eq!(client.recv().await, Some(ServerFrame::end(RequestId::new(2))));

    stop.send(()).unwrap();
    server.await.unwrap();
    assert_eq!(client.recv().await, None, "shutdown closes open connections");
    assert!(!path.exists(), "the socket file goes away with the listener");
}
