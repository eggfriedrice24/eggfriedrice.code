use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};

use pretty_assertions::assert_eq;

use super::LinkedFile;
use crate::StdxError;

#[test]
fn a_file_is_read_and_written_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "a = 1\n").unwrap();
    let file = LinkedFile::open(&path).unwrap();
    assert_eq!(file.path(), path);
    assert_eq!(file.target(), path);
    assert_eq!(file.text(), Some("a = 1\n"));
    file.write_if_unchanged(b"a = 2\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "a = 2\n");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn a_link_is_followed_and_stays_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles/config.toml");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "a = 1\n").unwrap();
    let link = dir.path().join("config.toml");
    symlink(&real, &link).unwrap();
    let file = LinkedFile::open(&link).unwrap();
    assert_eq!(file.target(), fs::canonicalize(&real).unwrap());
    assert_eq!(file.text(), Some("a = 1\n"));
    file.write_if_unchanged(b"a = 2\n").unwrap();
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(&real).unwrap(), "a = 2\n");
}

#[test]
fn a_link_to_nothing_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("config.toml");
    let gone = dir.path().join("gone.toml");
    symlink(&gone, &link).unwrap();
    let error = LinkedFile::open(&link).unwrap_err();
    assert!(
        matches!(&error, StdxError::DanglingLink { path, target } if *path == link && *target == gone),
        "{error:?}"
    );
}

#[test]
fn a_missing_file_is_created_with_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("efr/projects.toml");
    let file = LinkedFile::open(&path).unwrap();
    assert_eq!(file.text(), None);
    file.write_if_unchanged(b"x\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "x\n");
}

#[test]
fn a_file_that_changed_since_the_read_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "a = 1\n").unwrap();
    let file = LinkedFile::open(&path).unwrap();
    fs::write(&path, "a = 3\n").unwrap();
    let error = file.write_if_unchanged(b"a = 2\n").unwrap_err();
    assert!(matches!(&error, StdxError::FileChanged { path: changed } if *changed == path));
    assert_eq!(fs::read_to_string(&path).unwrap(), "a = 3\n");
}

#[test]
fn a_file_or_a_link_that_appeared_since_the_read_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let file = LinkedFile::open(&path).unwrap();
    fs::write(&path, "theirs\n").unwrap();
    let error = file.write_if_unchanged(b"mine\n").unwrap_err();
    assert!(matches!(error, StdxError::FileChanged { .. }), "{error:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\n");

    // A link to nothing reads as no file, but it is somebody's change all the same.
    fs::remove_file(&path).unwrap();
    let file = LinkedFile::open(&path).unwrap();
    symlink(dir.path().join("gone.toml"), &path).unwrap();
    let error = file.write_if_unchanged(b"mine\n").unwrap_err();
    assert!(matches!(error, StdxError::FileChanged { .. }), "{error:?}");
    assert!(!dir.path().join("gone.toml").exists());
}
