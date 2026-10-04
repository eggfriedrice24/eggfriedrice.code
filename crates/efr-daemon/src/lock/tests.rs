use std::os::unix::fs::PermissionsExt as _;

use crate::DaemonError;
use crate::lock::DaemonLock;

#[test]
fn the_first_daemon_takes_the_lock_and_creates_its_directory() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("data/efr/daemon.lock");

    let lock = DaemonLock::acquire(&path).unwrap();

    assert_eq!(lock.path(), path);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
}

#[test]
fn a_second_daemon_is_refused_while_the_first_holds_the_lock() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("daemon.lock");
    let _first = DaemonLock::acquire(&path).unwrap();

    let second = DaemonLock::acquire(&path);

    assert!(
        matches!(&second, Err(DaemonError::AlreadyRunning { path: held }) if *held == path),
        "{second:?}"
    );
}

#[test]
fn the_lock_is_free_again_once_the_holder_drops_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("daemon.lock");
    drop(DaemonLock::acquire(&path).unwrap());

    assert!(DaemonLock::acquire(&path).is_ok());
}
