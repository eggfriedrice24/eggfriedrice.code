use std::fs::File;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use efr_sandbox::{ATTACH_DIR, JOB_DIR};
use pretty_assertions::assert_eq;

use super::{make, marker, open, remove, write_new};

/// A file with `bytes`, at its start.
fn source(dir: &Path, bytes: &[u8]) -> File {
    let path = dir.join("source");
    File::create(&path).unwrap().write_all(bytes).unwrap();
    File::open(path).unwrap()
}

#[test]
fn a_contained_call_cannot_plant_an_owned_dir_before_efrd_makes_it() {
    // NOTE: efrd makes the dirs before every contained call, with their markers, so the
    // plan of the call finds them and keeps them read-only for the call.
    for name in [JOB_DIR, ATTACH_DIR] {
        let scratch = tempfile::tempdir().unwrap();
        make(scratch.path(), name).unwrap();
        let dir = scratch.path().join(name);
        let meta = std::fs::symlink_metadata(&dir).unwrap();
        assert!(meta.is_dir());
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        assert!(dir.join(marker(name)).is_file());
        std::fs::write(dir.join("kept"), "kept").unwrap();
        make(scratch.path(), name).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("kept")).unwrap(), "kept", "made once");
        // A scratch that does not exist yet holds nothing to make.
        make(&scratch.path().join("not-yet"), name).unwrap();
        assert!(!scratch.path().join("not-yet").exists());
    }
}

#[test]
fn a_dir_or_a_link_that_efrd_did_not_make_goes_aside() {
    let scratch = tempfile::tempdir().unwrap();
    let jobs = scratch.path().join(JOB_DIR);
    std::fs::create_dir(&jobs).unwrap();
    std::fs::write(jobs.join("j1.out"), "planted").unwrap();
    make(scratch.path(), JOB_DIR).unwrap();
    assert!(jobs.join(marker(JOB_DIR)).is_file());
    assert!(!jobs.join("j1.out").exists());
    let aside = scratch.path().join("jobs.moved-1");
    assert_eq!(std::fs::read_to_string(aside.join("j1.out")).unwrap(), "planted");
    // A link goes aside too, and its target is untouched.
    let other = tempfile::tempdir().unwrap();
    let target = other.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, other.path().join(ATTACH_DIR)).unwrap();
    make(other.path(), ATTACH_DIR).unwrap();
    assert!(other.path().join("attachments.moved-1").is_symlink());
    assert!(!target.join(marker(ATTACH_DIR)).exists());
    assert!(other.path().join(ATTACH_DIR).join(marker(ATTACH_DIR)).is_file());
}

#[test]
fn a_new_file_has_the_bytes_of_its_source_and_mode_0600() {
    let scratch = tempfile::tempdir().unwrap();
    make(scratch.path(), ATTACH_DIR).unwrap();
    let dir = open(scratch.path(), ATTACH_DIR).unwrap();
    let bytes: Vec<u8> = (0..=255_u8).cycle().take(300_000).collect();
    write_new(&dir, "a1-shot.png", &source(scratch.path(), &bytes)).unwrap();
    let copy = scratch.path().join(ATTACH_DIR).join("a1-shot.png");
    assert_eq!(std::fs::read(&copy).unwrap(), bytes);
    assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn a_source_on_another_file_system_is_copied_from_its_position() {
    // NOTE: a memfd lives in a file system of its own, so `copy_file_range` to the
    // owned dir always fails with `EXDEV`, and the copy reads and writes.
    let scratch = tempfile::tempdir().unwrap();
    make(scratch.path(), ATTACH_DIR).unwrap();
    let dir = open(scratch.path(), ATTACH_DIR).unwrap();
    let memfd = rustix::fs::memfd_create("efr-source", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut memfd = File::from(memfd);
    let bytes: Vec<u8> = (0..=255_u8).cycle().take(200_000).collect();
    memfd.write_all(&bytes).unwrap();
    memfd.seek(SeekFrom::Start(7)).unwrap();
    write_new(&dir, "a2-notes.txt", &memfd).unwrap();
    let copy = scratch.path().join(ATTACH_DIR).join("a2-notes.txt");
    assert_eq!(std::fs::read(copy).unwrap(), bytes[7..]);
}

#[test]
fn a_planted_link_with_the_next_name_goes_aside_and_its_target_stays() {
    let scratch = tempfile::tempdir().unwrap();
    make(scratch.path(), ATTACH_DIR).unwrap();
    let copies = scratch.path().join(ATTACH_DIR);
    let target = scratch.path().join("target");
    std::fs::write(&target, "the user's file").unwrap();
    std::os::unix::fs::symlink(&target, copies.join("a2-shot.png")).unwrap();
    std::fs::write(copies.join("a3-notes.txt"), "planted").unwrap();
    let dir = open(scratch.path(), ATTACH_DIR).unwrap();

    write_new(&dir, "a2-shot.png", &source(scratch.path(), b"png")).unwrap();
    write_new(&dir, "a3-notes.txt", &source(scratch.path(), b"notes")).unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "the user's file");
    assert!(copies.join("a2-shot.png.moved-1").is_symlink());
    assert!(!copies.join("a2-shot.png").is_symlink());
    assert_eq!(std::fs::read(copies.join("a2-shot.png")).unwrap(), b"png");
    assert_eq!(std::fs::read(copies.join("a3-notes.txt.moved-1")).unwrap(), b"planted");
    assert_eq!(std::fs::read(copies.join("a3-notes.txt")).unwrap(), b"notes");
}

#[test]
fn a_name_of_the_longest_attachment_fits() {
    let scratch = tempfile::tempdir().unwrap();
    make(scratch.path(), ATTACH_DIR).unwrap();
    let dir = open(scratch.path(), ATTACH_DIR).unwrap();
    let name = format!("a1-{}", "x".repeat(efr_protocol::NAME_MAX_BYTES));
    write_new(&dir, &name, &source(scratch.path(), b"x")).unwrap();
    assert_eq!(std::fs::read(scratch.path().join(ATTACH_DIR).join(&name)).unwrap(), b"x");
}

#[test]
fn a_dir_that_a_link_turns_does_not_open() {
    let scratch = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), scratch.path().join(ATTACH_DIR)).unwrap();
    assert!(open(scratch.path(), ATTACH_DIR).is_err());
}

#[test]
fn remove_takes_a_link_and_never_its_target() {
    let scratch = tempfile::tempdir().unwrap();
    make(scratch.path(), ATTACH_DIR).unwrap();
    let copies = scratch.path().join(ATTACH_DIR);
    let target = scratch.path().join("target");
    std::fs::write(&target, "kept").unwrap();
    std::os::unix::fs::symlink(&target, copies.join("a1-x")).unwrap();
    let dir = open(scratch.path(), ATTACH_DIR).unwrap();
    remove(&dir, "a1-x").unwrap();
    remove(&dir, "a1-x").unwrap();
    assert!(!copies.join("a1-x").exists());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "kept");
}
