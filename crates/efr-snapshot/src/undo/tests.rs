use efr_protocol::UndoReason;
use pretty_assertions::assert_eq;

use super::current::Current;
use super::decide::{Facts, Side, decide};
use super::{Expect, PlanAction, valid_undo_id};
use crate::capture::LeftOutReason;
use crate::object::{blob_id, link_id};

fn file(text: &str) -> Current {
    Current::File { id: blob_id(text.as_bytes()), mode: 0o644 }
}

fn blob(text: &str) -> Side {
    Side::Blob { id: blob_id(text.as_bytes()), mode: 0o100_644 }
}

fn turn_path() -> Facts {
    Facts { touched: true, has_post: true, ..Facts::default() }
}

fn action(target: &Side, source: &Side, current: &Current, facts: Facts) -> PlanAction {
    decide("a", target, source, current, None, facts).action
}

#[test]
fn a_path_as_the_turn_left_it_goes_back() {
    let entry = decide("a", &blob("old"), &blob("new"), &file("new"), None, turn_path());
    assert_eq!(entry.action, PlanAction::Write { blob: blob_id(b"old"), mode: 0o644 });
    assert_eq!(entry.expect, Expect::Blob(blob_id(b"new")));
    assert_eq!(entry.trash, Some((blob_id(b"new"), 0o100_644)));
    let new = decide("a", &Side::Missing, &blob("new"), &file("new"), None, turn_path());
    assert_eq!(new.action, PlanAction::Delete);
    assert!(new.trash.is_some(), "a removal goes to the undo trash");
    let gone = decide("a", &blob("old"), &Side::Missing, &Current::Missing, None, turn_path());
    assert_eq!(gone.trash, None, "nothing to keep where nothing is");
}

#[test]
fn the_checks_come_in_their_order() {
    let left = Side::LeftOut(LeftOutReason::TooLarge);
    let forced = Facts { force: true, ..turn_path() };
    assert_eq!(
        action(&left, &blob("new"), &Current::Unknown, forced),
        PlanAction::NotCovered(LeftOutReason::TooLarge),
        "a left out path is never touched"
    );
    let outside = Facts { touched: false, force: true, ..turn_path() };
    assert_eq!(
        action(&blob("old"), &blob("new"), &file("new"), outside),
        PlanAction::Skip(UndoReason::OutsideTurnCalls),
        "force does not reach a path that no call changed"
    );
    assert_eq!(
        action(&blob("old"), &blob("new"), &Current::Unknown, forced),
        PlanAction::Skip(UndoReason::Unknown)
    );
    assert_eq!(
        action(&blob("old"), &blob("new"), &file("old"), turn_path()),
        PlanAction::Skip(UndoReason::AlreadyDone)
    );
    assert_eq!(
        action(&blob("old"), &blob("new"), &file("mine"), turn_path()),
        PlanAction::Skip(UndoReason::ChangedAfterTurn)
    );
    assert_eq!(
        action(&blob("old"), &blob("new"), &file("mine"), Facts { later: true, ..turn_path() }),
        PlanAction::Skip(UndoReason::LaterTurnNotUndone)
    );
    assert_eq!(
        action(&blob("old"), &blob("new"), &file("new"), Facts { has_post: false, ..turn_path() }),
        PlanAction::Skip(UndoReason::NoRecordAfterTurn)
    );
    assert_eq!(
        action(&blob("old"), &blob("new"), &file("new"), Facts { replaced: true, ..turn_path() }),
        PlanAction::Skip(UndoReason::RootReplaced)
    );
}

#[test]
fn force_names_what_it_overrides() {
    let forced = Facts { force: true, ..turn_path() };
    let entry = decide("a", &blob("old"), &blob("new"), &file("mine"), None, forced);
    assert_eq!(entry.forced, Some(UndoReason::ChangedAfterTurn));
    assert_eq!(entry.trash, Some((blob_id(b"mine"), 0o100_644)), "the user's bytes are kept");
    let maybe = Facts {
        touched: false,
        maybe_touched: Some(UndoReason::SnapshotFailed),
        force: true,
        ..turn_path()
    };
    let entry = decide("a", &blob("old"), &blob("new"), &file("new"), None, maybe);
    assert_eq!(entry.forced, Some(UndoReason::SnapshotFailed));
    let without = Facts { force: false, ..maybe };
    assert_eq!(
        action(&blob("old"), &blob("new"), &file("new"), without),
        PlanAction::Skip(UndoReason::SnapshotFailed)
    );
}

#[test]
fn the_executable_bit_counts_and_links_keep_their_target() {
    let exec = Side::Blob { id: blob_id(b"x"), mode: 0o100_755 };
    let plain = Current::File { id: blob_id(b"x"), mode: 0o644 };
    let entry = decide("a", &exec, &blob("x"), &plain, None, turn_path());
    assert_eq!(entry.action, PlanAction::Write { blob: blob_id(b"x"), mode: 0o755 });
    let link = Side::Link { id: link_id(b"/target") };
    let entry = decide("a", &link, &blob("x"), &plain, Some(b"/target"), turn_path());
    assert_eq!(entry.action, PlanAction::Link { target: b"/target".to_vec() });
    let now = Current::Link { target: b"/target".to_vec() };
    assert_eq!(
        action(&link, &blob("x"), &now, turn_path()),
        PlanAction::Skip(UndoReason::AlreadyDone)
    );
    assert_eq!(now.trash(), Some((link_id(b"/target"), 0o120_000)));
}

#[test]
fn undo_ids_name_a_ref_and_a_directory() {
    assert!(valid_undo_id("u-0192f8a0-1b2c-7d3e-8f40-123456789abc"));
    assert!(valid_undo_id("0192f8a0-1b2c-7d3e-8f40-123456789abc"));
    let long = "x".repeat(81);
    for bad in ["", "..", "a/b", "-x", ".x", "a b", "a..b", long.as_str()] {
        assert!(!valid_undo_id(bad), "{bad:?}");
    }
}

#[test]
fn a_read_of_the_planner_never_waits_on_a_fifo_or_follows_a_link() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    nix::unistd::mkfifo(&dir.join("fifo"), nix::sys::stat::Mode::S_IRWXU).unwrap();
    std::os::unix::fs::symlink(dir.join("a.txt"), dir.join("link")).unwrap();

    assert!(super::open_regular(&dir.join("a.txt")).is_ok());
    // NOTE: a fifo with no writer would block a plain open for read forever.
    assert!(super::open_regular(&dir.join("fifo")).is_err(), "a fifo is no regular file");
    assert!(super::open_regular(&dir.join("link")).is_err(), "a link at the end is refused");
}
