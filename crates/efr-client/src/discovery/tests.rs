use std::fs;
use std::path::{Path, PathBuf};

use efr_protocol::PROTOCOL_VERSION;
use efr_stdx::paths::Dirs;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{DaemonInfo, Discovered, decide, discover, read_daemon_json};
use crate::ClientError;

fn dirs(root: &Path) -> Dirs {
    Dirs::new(root.join("config"), root.join("data"), root.join("state"), root.join("run")).unwrap()
}

fn info(protocol: u32, socket: &str) -> DaemonInfo {
    DaemonInfo {
        pid: 4242,
        socket: PathBuf::from(socket),
        protocol,
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        tailnet_endpoint: None,
    }
}

#[test]
fn the_decision_table() {
    let default = PathBuf::from("/run/user/1000/efr/daemon.sock");
    let json_path = Path::new("/run/user/1000/efr/daemon.json");

    let none = decide(default.clone(), json_path, None).unwrap();
    assert_eq!(none, Discovered { socket: default.clone(), info: None });

    let same = info(PROTOCOL_VERSION, "/tmp/efr-test/daemon.sock");
    let found = decide(default.clone(), json_path, Some(same.clone())).unwrap();
    assert_eq!(found, Discovered { socket: same.socket.clone(), info: Some(same) });

    let newer = decide(default.clone(), json_path, Some(info(PROTOCOL_VERSION + 1, "/x.sock")));
    assert!(matches!(
        newer,
        Err(ClientError::ProtocolMismatch { daemon, client })
            if daemon == PROTOCOL_VERSION + 1 && client == PROTOCOL_VERSION
    ));

    let relative = decide(default, json_path, Some(info(PROTOCOL_VERSION, "daemon.sock")));
    assert!(matches!(
        relative,
        Err(ClientError::RelativeSocket { ref socket, .. }) if socket == Path::new("daemon.sock")
    ));
}

#[tokio::test]
async fn without_daemon_json_the_default_socket_is_used() {
    let root = tempfile::tempdir().unwrap();
    let dirs = dirs(root.path());
    let found = discover(&dirs).await.unwrap();
    assert_eq!(found, Discovered { socket: dirs.socket_path(), info: None });
}

#[tokio::test]
async fn daemon_json_names_the_socket() {
    let root = tempfile::tempdir().unwrap();
    let dirs = dirs(root.path());
    fs::create_dir_all(dirs.runtime()).unwrap();
    let socket = root.path().join("elsewhere.sock");
    let written = json!({
        "pid": 4242,
        "socket": socket,
        "protocol": PROTOCOL_VERSION,
        "daemon_id": "019a9b1c-3d00-7a10-8b20-000000000007",
        "a_later_member": true,
    });
    fs::write(dirs.daemon_json_path(), serde_json::to_vec(&written).unwrap()).unwrap();
    let found = discover(&dirs).await.unwrap();
    assert_eq!(found.socket, socket);
    assert_eq!(found.info.unwrap().pid, 4242);
}

#[tokio::test]
async fn a_daemon_json_that_is_not_json_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let dirs = dirs(root.path());
    fs::create_dir_all(dirs.runtime()).unwrap();
    fs::write(dirs.daemon_json_path(), b"{\"pid\": ").unwrap();
    let error = discover(&dirs).await.unwrap_err();
    assert!(matches!(error, ClientError::InvalidDaemonJson { ref path, .. }
        if *path == dirs.daemon_json_path()));
}

#[tokio::test]
async fn an_unreadable_daemon_json_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    // A directory where the file belongs cannot be read as a file.
    let path = root.path().join("daemon.json");
    fs::create_dir(&path).unwrap();
    let error = read_daemon_json(&path).await.unwrap_err();
    assert!(matches!(error, ClientError::ReadDaemonJson { .. }));
}

#[test]
fn daemon_info_round_trips_without_the_optional_endpoint() {
    let value = serde_json::to_value(info(1, "/run/user/1000/efr/daemon.sock")).unwrap();
    assert_eq!(
        value,
        json!({
            "pid": 4242,
            "socket": "/run/user/1000/efr/daemon.sock",
            "protocol": 1,
            "daemon_id": "019a9b1c-3d00-7a10-8b20-000000000007",
        })
    );
    let back: DaemonInfo = serde_json::from_value(value).unwrap();
    assert_eq!(back, info(1, "/run/user/1000/efr/daemon.sock"));
}
