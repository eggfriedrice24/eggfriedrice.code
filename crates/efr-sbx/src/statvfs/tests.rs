//! The paths that `efr-sbx statvfs` reads, and its answers.

use std::path::{Path, PathBuf};

use efr_sandbox::MountSizes;

use super::*;

#[test]
fn statvfs_reads_nul_separated_paths_within_its_limits() {
    let paths = read_paths(&b"/\0/tmp\0\0/a b\0"[..]).unwrap();
    assert_eq!(paths, [PathBuf::from("/"), PathBuf::from("/tmp"), PathBuf::from("/a b")]);
    assert_eq!(read_paths(&b""[..]).unwrap(), Vec::<PathBuf>::new());

    let long = vec![b'a'; MAX_STATVFS_PATH_BYTES + 1];
    let error = read_paths(&long[..]).unwrap_err();
    assert!(matches!(error, SbxError::StatvfsInput { .. }), "{error:?}");

    let many: Vec<u8> = (0..=MAX_STATVFS_PATHS).flat_map(|_| b"/\0".iter().copied()).collect();
    let error = read_paths(&many[..]).unwrap_err();
    assert!(matches!(error, SbxError::StatvfsInput { .. }), "{error:?}");
}

#[test]
fn statvfs_answers_with_sizes_or_the_error_number() {
    let root = measure("/".to_owned(), Path::new("/"));
    assert_eq!(root.path, "/");
    match root.sizes() {
        MountSizes::Measured { blocks, frsize, .. } => assert!(blocks > 0 && frsize > 0),
        other => panic!("the root file system has sizes: {other:?}"),
    }
    let missing = measure("/no/such/dir".to_owned(), Path::new("/no/such/dir"));
    assert_eq!(missing.sizes(), MountSizes::Failed { errno: 2 });
    let line = serde_json::to_string(&missing).unwrap();
    assert_eq!(line, r#"{"path":"/no/such/dir","errno":2}"#);
}
