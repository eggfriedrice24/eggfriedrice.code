//! The worktree registration record: read at registration, compared before each call.

use efr_sandbox::WorktreeCheck;

use super::{check, load, register};

/// A main repository with a worktree at `<root>/wt`, as `git worktree add` leaves it.
fn worktree(root: &std::path::Path) -> std::path::PathBuf {
    let main = root.join("main/.git/worktrees/wt");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(main.join("commondir"), "../..\n").unwrap();
    let wt = root.join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", main.display())).unwrap();
    wt
}

#[test]
fn a_worktree_is_recorded_and_matches_until_its_git_file_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = root.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let wt = worktree(&root);
    let record = register(&state, &wt).unwrap().unwrap();
    assert_eq!(record.git_dir, root.join("main/.git/worktrees/wt"));
    assert_eq!(record.common_dir, root.join("main/.git"));
    assert_eq!(load(&state, &wt), Some(record.clone()));
    assert_eq!(check(&state, &wt), WorktreeCheck::Matches(record));
    // A call rewrites the .git file to point elsewhere.
    let elsewhere = root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", elsewhere.display())).unwrap();
    assert_eq!(check(&state, &wt), WorktreeCheck::Changed);
}

#[test]
fn a_project_without_a_record_or_without_a_git_file_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = root.join("state");
    let wt = worktree(&root);
    assert_eq!(check(&state, &wt), WorktreeCheck::NoRecord);
    let plain = root.join("plain");
    std::fs::create_dir_all(plain.join(".git")).unwrap();
    assert_eq!(register(&state, &plain).unwrap(), None);
    assert_eq!(check(&state, &plain), WorktreeCheck::NotWorktree);
}
