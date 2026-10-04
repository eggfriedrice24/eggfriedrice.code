use std::path::PathBuf;

use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::DaemonError;
use crate::discovery::{self, DAEMON_ID_FILE, DaemonInfo};

fn info(pid: u32) -> DaemonInfo {
    DaemonInfo {
        pid,
        socket: PathBuf::from("/run/user/1000/efr/daemon.sock"),
        protocol: 1,
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        tailnet_endpoint: None,
    }
}

#[test]
fn daemon_json_has_the_shape_the_client_reads() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("daemon.json");

    discovery::write(&path, &info(4242)).unwrap();

    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        written,
        json!({
            "pid": 4242,
            "socket": "/run/user/1000/efr/daemon.sock",
            "protocol": 1,
            "daemon_id": "019a9b1c-3d00-7a10-8b20-000000000007",
        })
    );
}

#[test]
fn a_daemon_removes_only_its_own_daemon_json() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("daemon.json");
    discovery::write(&path, &info(7)).unwrap();

    discovery::remove(&path, 8);
    assert!(path.exists(), "another daemon's file stays");

    discovery::remove(&path, 7);
    assert!(!path.exists());
}

#[test]
fn the_daemon_id_is_minted_once_and_kept() {
    let root = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let rng = TestRng::new(1);

    let first = discovery::daemon_id(root.path(), &clock, &rng).unwrap();
    let again = discovery::daemon_id(root.path(), &clock, &TestRng::new(2)).unwrap();

    assert_eq!(first, again);
    let stored = std::fs::read_to_string(root.path().join(DAEMON_ID_FILE)).unwrap();
    assert_eq!(stored.trim(), first.to_string());
}

#[test]
fn a_malformed_daemon_id_is_an_error_not_a_new_identity() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(DAEMON_ID_FILE), "not a uuid\n").unwrap();

    let result = discovery::daemon_id(root.path(), &TestClock::new(), &TestRng::new(1));

    assert!(matches!(result, Err(DaemonError::InvalidDaemonId { .. })), "{result:?}");
}
