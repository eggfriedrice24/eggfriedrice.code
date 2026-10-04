use std::fs;
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{Claim, claim_dir, create_private, parent_dir, write_atomic};
use crate::StdxError;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn write_atomic_creates_a_private_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.json");
    write_atomic(&path, b"{\"pid\":1}").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"{\"pid\":1}");
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn write_atomic_replaces_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("openai.json");
    fs::write(&path, b"old content that is longer").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    write_atomic(&path, b"new").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn write_atomic_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    write_atomic(&path, b"1").unwrap();
    write_atomic(&path, b"2").unwrap();
    assert_eq!(entries(dir.path()), ["state.json"]);
}

#[test]
fn write_atomic_needs_a_file_name() {
    for path in ["/", "/tmp/.."] {
        let err = write_atomic(Path::new(path), b"x").unwrap_err();
        assert!(matches!(err, StdxError::NoFileName { .. }), "{path}: {err:?}");
    }
}

#[test]
fn write_atomic_needs_the_parent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let err = write_atomic(&dir.path().join("missing/file"), b"x").unwrap_err();
    match err {
        StdxError::CreateFile { source, .. } => assert_eq!(source.kind(), ErrorKind::NotFound),
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(entries(dir.path()), Vec::<String>::new());
}

#[test]
fn failed_rename_removes_the_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("taken");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("inside"), b"x").unwrap();
    let err = write_atomic(&target, b"x").unwrap_err();
    assert!(matches!(err, StdxError::Rename { .. }), "{err:?}");
    assert_eq!(entries(dir.path()), ["taken"]);
}

#[test]
fn concurrent_writes_never_mix() {
    const WRITERS: u8 = 4;
    const WRITES: usize = 10;
    const LEN: usize = 64 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared");
    write_atomic(&path, &[0; LEN]).unwrap();
    std::thread::scope(|scope| {
        for writer in 1..=WRITERS {
            let path = &path;
            scope.spawn(move || {
                for _ in 0..WRITES {
                    write_atomic(path, &[writer; LEN]).unwrap();
                }
            });
        }
        let path = &path;
        scope.spawn(move || {
            for _ in 0..WRITES * usize::from(WRITERS) {
                let seen = fs::read(path).unwrap();
                assert_eq!(seen.len(), LEN);
                assert!(seen.iter().all(|byte| *byte == seen[0]), "a read saw mixed content");
            }
        });
    });
    let last = fs::read(&path).unwrap();
    assert!((1..=WRITERS).contains(&last[0]));
    assert!(last.iter().all(|byte| *byte == last[0]));
    assert_eq!(entries(dir.path()), ["shared"]);
}

#[test]
fn parent_of_a_bare_name_is_the_current_directory() {
    assert_eq!(parent_dir(Path::new("file")), Path::new("."));
    assert_eq!(parent_dir(Path::new("/srv/file")), Path::new("/srv"));
    assert_eq!(parent_dir(Path::new("dir/file")), Path::new("dir"));
}

#[test]
fn create_private_makes_a_writable_0600_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("segment.rec");
    let mut file = create_private(&path).unwrap();
    file.write_all(b"bytes").unwrap();
    drop(file);
    assert_eq!(fs::read(&path).unwrap(), b"bytes");
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn create_private_refuses_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exists");
    fs::write(&path, b"keep").unwrap();
    match create_private(&path).unwrap_err() {
        StdxError::CreateFile { source, .. } => assert_eq!(source.kind(), ErrorKind::AlreadyExists),
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(fs::read(&path).unwrap(), b"keep");
}

#[test]
fn create_private_does_not_follow_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let link = dir.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(create_private(&link).is_err());
    assert!(!target.exists());
}

#[test]
fn claim_dir_claims_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("2026-10-04-fix-dns-0a1b");
    assert_eq!(claim_dir(&path).unwrap(), Claim::Claimed);
    assert_eq!(claim_dir(&path).unwrap(), Claim::Taken);
    assert_eq!(mode(&path), 0o700);
}

#[test]
fn claim_dir_counts_a_file_as_taken() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"").unwrap();
    assert_eq!(claim_dir(&path).unwrap(), Claim::Taken);
}

#[test]
fn claim_dir_does_not_create_parents() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("missing");
    match claim_dir(&parent.join("child")).unwrap_err() {
        StdxError::CreateDir { source, .. } => assert_eq!(source.kind(), ErrorKind::NotFound),
        other => panic!("unexpected error {other:?}"),
    }
    assert!(!parent.exists());
}

#[test]
fn concurrent_claims_have_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("contested");
    let claims: Vec<Claim> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8).map(|_| scope.spawn(|| claim_dir(&path).unwrap())).collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });
    assert_eq!(claims.iter().filter(|claim| **claim == Claim::Claimed).count(), 1);
}
