use std::path::Path;

use super::{REFERENCE, SCHEMA, generated, run};

#[test]
fn a_check_of_a_tree_without_the_files_fails_and_writing_makes_them_current() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();

    assert!(!run(root.path(), true).unwrap());
    assert!(run(root.path(), false).unwrap());
    assert!(run(root.path(), true).unwrap());
    for path in [REFERENCE, SCHEMA] {
        assert!(root.path().join(path).is_file(), "{path}");
    }
}

#[test]
fn the_committed_files_are_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, text) in generated().unwrap() {
        let on_disk = std::fs::read_to_string(root.join(path)).unwrap();
        assert!(on_disk == text, "{path} is not current; run `cargo xtask config-docs`");
    }
}
