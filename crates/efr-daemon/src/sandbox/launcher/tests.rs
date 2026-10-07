//! The launcher's copy: where efrd finds the installed launcher, the copy's mode and
//! the SHA-256 check.

use std::os::unix::fs::PermissionsExt as _;

use super::{LAUNCHER, find_source, install, matches};

#[test]
fn the_launcher_is_found_next_to_efrd_or_in_lib_efr() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let efrd = bin.join("efrd");
    assert_eq!(find_source(&efrd), None);
    let lib = root.join("lib/efr");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join(LAUNCHER), "lib").unwrap();
    assert_eq!(find_source(&efrd), Some(lib.join(LAUNCHER)));
    std::fs::write(bin.join(LAUNCHER), "next").unwrap();
    assert_eq!(find_source(&efrd), Some(bin.join(LAUNCHER)), "next to efrd wins");
}

#[test]
fn the_copy_is_private_and_matches_its_source_until_either_changes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("efr-sbx");
    std::fs::write(&source, b"\x7fELF launcher").unwrap();
    let copy = dir.path().join("runtime/bin/efr-sbx");
    install(&source, &copy).unwrap();
    let mode = std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o500);
    let dir_mode = std::fs::metadata(copy.parent().unwrap()).unwrap().permissions().mode();
    assert_eq!(dir_mode & 0o777, 0o700);
    assert!(matches(&source, &copy));
    // A second start replaces the read-only copy.
    std::fs::write(&source, b"\x7fELF launcher 2").unwrap();
    assert!(!matches(&source, &copy));
    install(&source, &copy).unwrap();
    assert!(matches(&source, &copy));
    assert!(!matches(&source, &dir.path().join("missing")));
}
