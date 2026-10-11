use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{CoverResult, recorded, resolve, walk_target};
use crate::capture::{LeftOut, LeftOutReason};
use crate::project_git::GitDirs;

#[test]
fn the_record_names_a_path_or_a_dir_above_it() {
    let left = [
        LeftOut { path: "target".to_owned(), reason: LeftOutReason::SkippedDir },
        LeftOut { path: "big.bin".to_owned(), reason: LeftOutReason::TooLarge },
    ];
    assert_eq!(recorded(&left, "target/debug/app"), Some(LeftOutReason::SkippedDir));
    assert_eq!(recorded(&left, "big.bin"), Some(LeftOutReason::TooLarge));
    assert_eq!(recorded(&left, "targets/x"), None, "a prefix of a name is no dir");
    assert_eq!(recorded(&left, "src/a.rs"), None);
}

#[test]
fn a_target_is_resolved_without_its_last_part() {
    let base = tempfile::tempdir().unwrap();
    let real = fs::canonicalize(base.path()).unwrap();
    fs::create_dir(real.join("dir")).unwrap();
    std::os::unix::fs::symlink(real.join("dir"), real.join("link")).unwrap();
    assert_eq!(resolve(&real.join("link/./x/../y")), real.join("dir/y"));
    assert_eq!(resolve(&real.join("link")), real.join("link"), "a command acts on the link");
}

#[test]
fn the_walk_answers_for_the_root_the_git_dir_and_paths_outside() {
    let base = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(base.path()).unwrap().join("p");
    fs::create_dir_all(root.join(".git/hooks")).unwrap();
    fs::create_dir_all(root.join("src/nested/.git")).unwrap();
    fs::write(root.join("src/a.rs"), "a").unwrap();
    let dirs = GitDirs { git_dir: root.join(".git"), common_dir: root.join(".git") };
    let walk = |path: &Path| walk_target(&root, path, Some(&dirs), 100);
    assert_eq!(walk(&root), Err(CoverResult::IsRoot));
    assert_eq!(walk(&root.join("..")), Err(CoverResult::OutsideRoot));
    assert_eq!(walk(&root.join(".git/hooks")), Err(CoverResult::GitInternal));
    assert_eq!(walk(&root.join("src")), Err(CoverResult::LeftOut(LeftOutReason::NestedRepo)));
    assert_eq!(walk(&root.join("gone")), Ok(None));
    assert_eq!(
        walk(&root.join("src/a.rs")),
        Ok(Some(("src/a.rs".to_owned(), vec!["src/a.rs".to_owned()])))
    );
    assert_eq!(
        walk_target(&root, &root.join("src/a.rs"), None, 0),
        Ok(Some(("src/a.rs".to_owned(), vec!["src/a.rs".to_owned()]))),
        "a single file needs no walk"
    );
}
