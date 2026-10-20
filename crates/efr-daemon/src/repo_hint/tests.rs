use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{FILE, RepoHints, write};

fn at(hours: i64) -> Timestamp {
    "2026-10-20T00:00:00Z".parse::<Timestamp>().unwrap() + SignedDuration::from_hours(hours)
}

#[test]
fn a_repository_is_due_again_a_day_after_its_hint() {
    let dir = tempfile::tempdir().unwrap();
    let hints = RepoHints::load(dir.path(), at(0));
    let tool = Path::new("/home/u/p/tool");

    assert!(hints.due(tool, at(0)));
    hints.shown(tool.to_path_buf(), at(0));
    assert!(!hints.due(tool, at(0)));
    assert!(!hints.due(tool, at(23)));
    assert!(hints.due(tool, at(24)));
    assert!(hints.due(Path::new("/home/u/p/other"), at(1)), "each repository on its own");
}

#[test]
fn the_file_keeps_the_hints_for_a_restart_and_drops_old_ones() {
    let dir = tempfile::tempdir().unwrap();
    let hints = RepoHints::load(dir.path(), at(0));
    let (file, text) = hints.shown(PathBuf::from("/home/u/p/old"), at(0));
    write(&file, &text);
    let (file, text) = hints.shown(PathBuf::from("/home/u/p/tool"), at(10));
    write(&file, &text);

    assert_eq!(file, dir.path().join(FILE));
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let again = RepoHints::load(dir.path(), at(20));
    assert!(!again.due(Path::new("/home/u/p/tool"), at(20)));
    assert!(!again.due(Path::new("/home/u/p/old"), at(20)));

    // A write after a day drops the old entry; the load at start does too.
    let (file, text) = again.shown(PathBuf::from("/home/u/p/new"), at(30));
    write(&file, &text);
    let held: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let roots: Vec<&String> = held.as_object().unwrap().keys().collect();
    assert_eq!(roots, ["/home/u/p/new", "/home/u/p/tool"]);
    let later = RepoHints::load(dir.path(), at(40));
    assert!(later.due(Path::new("/home/u/p/tool"), at(40)));
}

#[test]
fn a_broken_file_holds_no_hint() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), "not json").unwrap();
    let hints = RepoHints::load(dir.path(), at(0));
    assert!(hints.due(Path::new("/home/u/p/tool"), at(0)));
}

#[test]
fn the_write_makes_a_missing_state_root() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let hints = RepoHints::load(&state, at(0));
    let (file, text) = hints.shown(PathBuf::from("/home/u/p/tool"), at(0));
    write(&file, &text);
    assert!(file.exists());
}
