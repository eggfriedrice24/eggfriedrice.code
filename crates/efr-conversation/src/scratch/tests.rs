use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use efr_protocol::ConversationId;
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestDirs, TestRng};
use jiff::civil::{Date, date};
use pretty_assertions::assert_eq;

use super::{MARKER, Scratch, names, slug};
use crate::ConversationError;

const DAY: Date = date(2026, 10, 4);

fn conversation(seed: u64) -> ConversationId {
    ConversationId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
}

fn hex(id: ConversationId) -> String {
    id.to_string().replace('-', "")
}

struct Root {
    _dirs: TestDirs,
    path: PathBuf,
}

fn root() -> Root {
    let dirs = TestDirs::new().expect("temporary directories");
    let path = dirs.dirs().data().join("scratch");
    Root { _dirs: dirs, path }
}

#[test]
fn names_try_longer_id_tails_then_counters() {
    let id = conversation(1);
    let hex = hex(id);
    let names = names(DAY, "fix-tests", id);

    assert_eq!(names.len(), 11);
    assert_eq!(names[0], format!("2026-10-04-fix-tests-{}", &hex[24..]));
    assert_eq!(names[1], format!("2026-10-04-fix-tests-{}", &hex[20..]));
    assert_eq!(names[2], format!("2026-10-04-fix-tests-{hex}"));
    assert_eq!(names[3], format!("2026-10-04-fix-tests-{hex}-2"));
    assert_eq!(names[10], format!("2026-10-04-fix-tests-{hex}-9"));
}

#[test]
fn a_name_without_a_slug_has_no_empty_word() {
    let id = conversation(1);
    assert_eq!(names(DAY, "", id)[0], format!("2026-10-04-{}", &hex(id)[24..]));
}

#[test]
fn a_slug_is_up_to_five_lowercase_ascii_words() {
    assert_eq!(slug("Fix the failing CI tests now, please"), "fix-the-failing-ci-tests");
    assert_eq!(slug("  why is /etc/hosts empty?? "), "why-is-etc-hosts-empty");
    assert_eq!(slug("north \u{2192} south"), "north-south");
    assert_eq!(slug("!!! ???"), "");
    assert_eq!(slug(""), "");
}

#[test]
fn a_slug_is_at_most_48_bytes() {
    let long = "a".repeat(60);
    assert_eq!(slug(&long), "a".repeat(48));
    let words = format!("{} {} {}", "b".repeat(30), "c".repeat(30), "d");
    assert_eq!(slug(&words), "b".repeat(30), "a word that does not fit ends the slug");
}

#[test]
fn the_first_name_is_claimed_private_and_marked() {
    let root = root();
    let id = conversation(1);
    let mut scratch = Scratch::new(&root.path, id);

    let path = scratch.ensure(DAY, Some("Fix tests")).expect("claimed");

    assert_eq!(path, root.path.join(&names(DAY, "fix-tests", id)[0]));
    let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let marker = std::fs::read_to_string(path.join(MARKER)).expect("marker");
    assert_eq!(marker.trim(), id.to_string());
    assert_eq!(scratch.ensure(DAY, Some("Fix tests")).expect("again"), path);
}

#[test]
fn after_a_restart_the_same_directory_is_found_again() {
    let root = root();
    let id = conversation(1);
    let first = Scratch::new(&root.path, id).ensure(DAY, Some("Fix tests")).expect("claimed");
    std::fs::write(first.join("notes.txt"), "kept").expect("write");

    let again = Scratch::new(&root.path, id).ensure(DAY, Some("Fix tests")).expect("found");

    assert_eq!(again, first);
    assert!(again.join("notes.txt").is_file(), "nothing was made anew");
}

#[test]
fn a_name_that_something_else_holds_falls_back_to_a_longer_tail() {
    let root = root();
    let id = conversation(1);
    let taken = names(DAY, "fix-tests", id);
    std::fs::create_dir_all(root.path.join(&taken[0])).expect("someone else's directory");

    let path = Scratch::new(&root.path, id).ensure(DAY, Some("Fix tests")).expect("claimed");

    assert_eq!(path, root.path.join(&taken[1]));
}

#[test]
fn a_symbolic_link_at_the_name_is_never_taken_as_scratch() {
    let root = root();
    let id = conversation(1);
    let taken = names(DAY, "fix-tests", id);
    let elsewhere = root.path.with_file_name("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("target");
    std::fs::write(elsewhere.join(MARKER), format!("{id}\n")).expect("forged marker");
    std::fs::create_dir_all(&root.path).expect("root");
    std::os::unix::fs::symlink(&elsewhere, root.path.join(&taken[0])).expect("link");

    let path = Scratch::new(&root.path, id).ensure(DAY, Some("Fix tests")).expect("claimed");

    assert_eq!(path, root.path.join(&taken[1]));
}

#[test]
fn a_missing_directory_is_made_again_under_the_same_name() {
    let root = root();
    let id = conversation(1);
    let mut scratch = Scratch::new(&root.path, id);
    let path = scratch.ensure(DAY, Some("Fix tests")).expect("claimed");
    std::fs::remove_dir_all(&root.path).expect("removed with its root");

    let again = scratch.ensure(DAY, Some("Fix tests")).expect("made again");

    assert_eq!(again, path);
    assert!(again.join(MARKER).is_file());
}

#[test]
fn when_every_name_is_taken_the_claim_fails() {
    let root = root();
    let id = conversation(1);
    for name in names(DAY, "fix-tests", id) {
        std::fs::create_dir_all(root.path.join(name)).expect("taken");
    }

    let error = Scratch::new(&root.path, id).ensure(DAY, Some("Fix tests")).unwrap_err();

    assert!(matches!(error, ConversationError::ScratchNamesExhausted { .. }), "{error:?}");
}

#[test]
fn a_root_that_cannot_be_made_is_an_error() {
    let root = root();
    let parent = root.path.parent().expect("parent").to_path_buf();
    std::fs::write(parent.join("file"), "x").expect("file");
    let mut scratch = Scratch::new(parent.join("file").join("scratch"), conversation(1));

    let error = scratch.ensure(DAY, None).unwrap_err();

    assert!(matches!(error, ConversationError::CreateScratchRoot { .. }), "{error:?}");
}
