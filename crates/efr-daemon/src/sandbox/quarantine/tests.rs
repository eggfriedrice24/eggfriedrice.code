//! Moving quarantined changes back after a "keep".

use std::path::PathBuf;

use efr_protocol::SurfaceChange;

use super::{INDEX, restore};

fn change(path: PathBuf, quarantined: bool) -> SurfaceChange {
    SurfaceChange { path, rule: "commondir_in_main_git_dir".to_owned(), key: None, quarantined }
}

#[test]
fn a_kept_change_moves_back_and_one_in_the_way_stays() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let git = root.join("p/.git");
    std::fs::create_dir_all(&git).unwrap();
    let quarantine = root.join("quarantine/call");
    std::fs::create_dir_all(&quarantine).unwrap();
    std::fs::write(quarantine.join("0-commondir"), "/elsewhere\n").unwrap();
    std::fs::write(quarantine.join("1-config.worktree"), "[core]\n").unwrap();
    let index = serde_json::json!([
        { "from": git.join("commondir"), "to": "0-commondir" },
        { "from": git.join("config.worktree"), "to": "1-config.worktree" },
    ]);
    std::fs::write(quarantine.join(INDEX), index.to_string()).unwrap();
    // Something is at the second place again: it is never overwritten.
    std::fs::write(git.join("config.worktree"), "mine").unwrap();
    let changes = [
        change(git.join("commondir"), true),
        change(git.join("config.worktree"), true),
        change(git.join("hooks/pre-commit"), false),
    ];
    let error = restore(&quarantine, &changes).unwrap_err();
    assert!(error.contains("config.worktree could not move back"), "{error}");
    assert_eq!(std::fs::read_to_string(git.join("commondir")).unwrap(), "/elsewhere\n");
    assert_eq!(std::fs::read_to_string(git.join("config.worktree")).unwrap(), "mine");
    assert!(quarantine.join("1-config.worktree").exists());
}

#[test]
fn a_change_without_an_entry_or_a_quarantine_stays() {
    let dir = tempfile::tempdir().unwrap();
    let missing = restore(&dir.path().join("none"), &[change("/p/.git/x".into(), true)]);
    assert!(missing.unwrap_err().contains("cannot be read"));
    std::fs::write(dir.path().join(INDEX), "[]").unwrap();
    let unknown = restore(dir.path(), &[change("/p/.git/x".into(), true)]).unwrap_err();
    assert!(unknown.contains("is not in the quarantine"), "{unknown}");
}

#[test]
fn a_change_whose_copy_was_cut_short_stays() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let git = root.join("p/.git");
    std::fs::create_dir_all(&git).unwrap();
    let quarantine = root.join("quarantine/call");
    std::fs::create_dir_all(quarantine.join("0-hooks")).unwrap();
    std::fs::write(quarantine.join("0-hooks/pre-commit"), "#!/bin/").unwrap();
    let index = serde_json::json!([{
        "from": git.join("hooks"),
        "to": "0-hooks",
        "truncated": [git.join("hooks/pre-commit")],
    }]);
    std::fs::write(quarantine.join(INDEX), index.to_string()).unwrap();
    let error = restore(&quarantine, &[change(git.join("hooks"), true)]).unwrap_err();
    assert!(error.contains("kept only the first bytes of"), "{error}");
    assert!(!git.join("hooks").exists());
    assert!(quarantine.join("0-hooks/pre-commit").exists());
}
