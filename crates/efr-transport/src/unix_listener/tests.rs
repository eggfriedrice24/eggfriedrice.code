use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use efr_protocol::{RequestId, ServerFrame};
use efr_stdx::paths::MAX_SOCKET_PATH;
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
    let (_listener, temp) = stage(&staging).unwrap();
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
    drop(std::os::unix::net::UnixListener::bind(staging.join("s")).unwrap());
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
    fs::write(staging.join("s"), b"not a socket").unwrap();
    let error = UnixListener::bind(&path).await.unwrap_err();
    assert!(matches!(error, TransportError::Inspect { path: ref p, .. } if *p == staging));
    assert_eq!(fs::read(staging.join("notes.txt")).unwrap(), b"precious");
    assert_eq!(fs::read(staging.join("s")).unwrap(), b"not a socket");
    assert!(!path.exists());
}

/// A directory below `root` whose `daemon.sock` path is exactly `length` bytes.
fn dir_for_socket_length(root: &Path, length: usize) -> std::path::PathBuf {
    let fixed = root.as_os_str().len() + "/".len() + "/daemon.sock".len();
    root.join("d".repeat(length - fixed))
}

#[tokio::test]
async fn a_socket_path_of_the_longest_length_binds_through_its_staging_directory() {
    let root = tempfile::tempdir().unwrap();
    let path = dir_for_socket_length(root.path(), MAX_SOCKET_PATH).join("daemon.sock");
    assert_eq!(path.as_os_str().len(), MAX_SOCKET_PATH);

    let listener = UnixListener::bind(&path).await.unwrap();

    assert_eq!(mode(&path), 0o600);
    let _client = UnixStream::connect(&path).await.unwrap();
    assert!(listener.accept().await.is_ok());
    assert!(!staging_dir(&path).exists());
}

#[test]
fn the_staged_socket_is_never_longer_than_a_daemon_socket() {
    let path = Path::new("/run/user/1000/efr/daemon.sock");
    let staged = staging_dir(path).join("s");
    // The longest pid has seven digits, the one of this process may have fewer.
    let longest = staged.as_os_str().len() + 7 - std::process::id().to_string().len();
    assert!(longest <= path.as_os_str().len(), "{} for {}", staged.display(), path.display());
}

#[tokio::test]
async fn a_socket_path_too_long_for_a_socket_fails_with_its_path_and_creates_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dir = dir_for_socket_length(root.path(), MAX_SOCKET_PATH + 1);
    let path = dir.join("daemon.sock");

    let error = UnixListener::bind(&path).await.unwrap_err();

    assert!(matches!(&error, TransportError::PathTooLong { path: p } if *p == path), "{error:?}");
    assert_eq!(
        error.to_string(),
        format!(
            "the socket path {} is {} bytes, more than the 107 a Unix socket holds",
            path.display(),
            MAX_SOCKET_PATH + 1
        )
    );
    assert!(!dir.exists(), "nothing is created for a path that cannot work");
}

// NOTE: every socket of this process in one directory stages under the same name, so
// the binds run on threads of their own and start together, round after round: two
// binds whose staging overlapped would remove each other's staged socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sockets_in_one_directory_bind_at_once() {
    const BINDS: usize = 8;
    const ROUNDS: usize = 25;
    let dir = tempfile::tempdir().unwrap();
    for round in 0..ROUNDS {
        let start = Arc::new(tokio::sync::Barrier::new(BINDS));
        let binds: Vec<_> = (0..BINDS)
            .map(|n| {
                let path = dir.path().join(format!("{round}-{n}.sock"));
                let start = Arc::clone(&start);
                tokio::spawn(async move {
                    start.wait().await;
                    let bound = UnixListener::bind(&path).await;
                    (path, bound)
                })
            })
            .collect();
        for bind in binds {
            let (path, listener) = bind.await.unwrap();
            assert!(listener.is_ok(), "round {round}, {}: {listener:?}", path.display());
            assert_eq!(mode(&path), 0o600);
        }
    }
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
