use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use crate::testing::FakeFs;
use crate::worktree::{
    WorktreeCheck, WorktreeRecord, check_worktree, parse_git_file, read_worktree, record_file_name,
};

fn worktree() -> FakeFs {
    let mut fs = FakeFs::new();
    fs.file("/p/wt/.git", "gitdir: /p/main/.git/worktrees/wt\n")
        .file("/p/main/.git/worktrees/wt/commondir", "../..\n")
        .file("/p/main/.git/config", "");
    fs
}

fn record() -> WorktreeRecord {
    WorktreeRecord {
        root: "/p/wt".into(),
        git_dir: "/p/main/.git/worktrees/wt".into(),
        common_dir: "/p/main/.git".into(),
    }
}

#[test]
fn a_git_file_names_its_git_dir() {
    assert_eq!(
        parse_git_file("gitdir: ../main/.git/modules/x\n"),
        Some(PathBuf::from("../main/.git/modules/x"))
    );
    assert_eq!(parse_git_file("nonsense"), None);
    assert_eq!(parse_git_file("gitdir:   \n"), None);
}

#[test]
fn registration_reads_the_git_dir_and_the_common_dir() {
    let fs = worktree();
    assert_eq!(read_worktree(Path::new("/p/wt"), &fs).unwrap(), Some(record()));
    assert_eq!(read_worktree(Path::new("/p/main"), &fs).unwrap(), None);
    let back = WorktreeRecord::from_json(&record().to_json().unwrap()).unwrap();
    assert_eq!(back, record());
    assert_eq!(record_file_name(Path::new("/home/u/p/wt")), "home%u%p%wt.json");
}

#[test]
fn a_matching_record_gives_the_git_dirs_and_a_rewritten_file_does_not() {
    let mut fs = worktree();
    let root = Path::new("/p/wt");
    assert_eq!(
        check_worktree(root, Some(&record()), &fs).unwrap(),
        WorktreeCheck::Matches(record())
    );
    assert_eq!(check_worktree(root, None, &fs).unwrap(), WorktreeCheck::NoRecord);
    // The model rewrites the .git file to a fake git dir whose commondir leads home.
    fs.file("/p/wt/.git", "gitdir: fake\n")
        .file("/p/wt/fake/commondir", "/home/u\n")
        .dir("/home/u");
    assert_eq!(check_worktree(root, Some(&record()), &fs).unwrap(), WorktreeCheck::Changed);
    assert_eq!(
        check_worktree(Path::new("/p/main"), Some(&record()), &fs).unwrap(),
        WorktreeCheck::NotWorktree
    );
}
