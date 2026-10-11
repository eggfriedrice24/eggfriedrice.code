//! Undo and redo of a turn's files, planned on the store and applied by a small applier
//! of the test that does what the daemon's does: it checks each path against the
//! plan's expectation, keeps the trash blob, and writes the blob of the store.

use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use efr_protocol::UndoReason;
use efr_snapshot::{
    Expect, LeftOutReason, PlanAction, PlanEntry, RootPlan, Snapshots, UndoPlan, blob_id,
    stack_blocker,
};
use pretty_assertions::assert_eq;

use crate::support::{World, conversation, limits, refs, root, store, turn, write};

/// The undo id of the `n`th undo of a test.
pub(crate) fn undo_id(n: u32) -> String {
    format!("u-0192f8a0-0000-7000-8000-{n:012}")
}

/// One turn `n` of conversation `c` with one call in `dir` that runs `call`.
pub(crate) async fn one_call_turn(
    world: &World,
    c: u128,
    n: u128,
    dir: &Path,
    call: impl FnOnce(),
) {
    let before =
        world.snapshots.before_call(conversation(c), turn(n), vec![root(dir)], limits()).await;
    call();
    world.snapshots.after_call(before, limits()).await;
    world.snapshots.turn_end(turn(n), limits()).await;
}

/// What `path` is now, as the plan's expectation names it.
pub(crate) fn expect_now(path: &Path) -> Expect {
    match fs::symlink_metadata(path) {
        Err(_) => Expect::Missing,
        Ok(meta) if meta.file_type().is_symlink() => {
            Expect::Link(fs::read_link(path).unwrap().as_os_str().as_bytes().to_vec())
        }
        Ok(meta) if meta.is_file() => Expect::Blob(blob_id(&fs::read(path).unwrap())),
        Ok(_) => Expect::Unknown,
    }
}

/// Applies the entries of `root` that act, as the daemon does; returns the trash
/// bytes that each one kept.
pub(crate) async fn apply_root(snapshots: &Snapshots, root: &RootPlan) -> Vec<(String, Vec<u8>)> {
    let mut trash = Vec::new();
    for entry in root.entries.iter().filter(|entry| entry.acts()) {
        let path = root.root.join(&entry.path);
        if expect_now(&path) != entry.expect {
            continue;
        }
        if let Some((id, _)) = &entry.trash {
            trash.push((entry.path.clone(), snapshots.blob(&root.root, id).await.unwrap()));
        }
        let existed = fs::symlink_metadata(&path).is_ok();
        match &entry.action {
            PlanAction::Write { blob, mode } => {
                let bytes = snapshots.blob(&root.root, blob).await.unwrap();
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
                    fs::remove_file(&path).unwrap();
                }
                fs::write(&path, bytes).unwrap();
                if !existed {
                    fs::set_permissions(&path, fs::Permissions::from_mode(*mode)).unwrap();
                }
            }
            PlanAction::Link { target } => {
                let _ = fs::remove_file(&path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(target), &path).unwrap();
            }
            PlanAction::Delete => fs::remove_file(&path).unwrap(),
            _ => unreachable!("only acting entries"),
        }
    }
    trash
}

/// Captures, applies and finishes `plan`; returns the trash bytes.
pub(crate) async fn apply(world: &World, plan: &UndoPlan) -> Vec<(String, Vec<u8>)> {
    world.snapshots.capture_before_undo(plan, limits()).await.unwrap();
    let mut trash = Vec::new();
    for root in &plan.roots {
        trash.extend(apply_root(&world.snapshots, root).await);
    }
    world.snapshots.finish_undo(plan, limits()).await.unwrap();
    trash
}

pub(crate) async fn plan(world: &World, n: u128, force: bool, undo: u32) -> UndoPlan {
    world
        .snapshots
        .plan_undo(conversation(1), turn(n), &[], force, &undo_id(undo), limits())
        .await
        .unwrap()
}

fn entry<'a>(plan: &'a UndoPlan, path: &str) -> &'a PlanEntry {
    plan.roots
        .iter()
        .flat_map(|root| &root.entries)
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("no entry for {path}: {plan:#?}"))
}

fn actions(plan: &UndoPlan) -> Vec<(String, PlanAction)> {
    plan.roots
        .iter()
        .flat_map(|root| &root.entries)
        .map(|entry| (entry.path.clone(), entry.action.clone()))
        .collect()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[tokio::test]
async fn undo_restores_modified_and_trashes_created() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "old\n");
    one_call_turn(&world, 1, 1, &dir, || {
        write(&dir.join("a.txt"), "new\n");
        write(&dir.join("notes.md"), "notes\n");
    })
    .await;

    let plan = plan(&world, 1, false, 1).await;
    assert_eq!(plan.undo_id, undo_id(1));
    assert!(!plan.redo && !plan.resumed);
    let restore = entry(&plan, "a.txt");
    assert_eq!(restore.action, PlanAction::Write { blob: blob_id(b"old\n"), mode: 0o644 });
    assert_eq!(restore.expect, Expect::Blob(blob_id(b"new\n")));
    assert_eq!(entry(&plan, "notes.md").action, PlanAction::Delete);

    let trash = apply(&world, &plan).await;
    assert_eq!(read(&dir.join("a.txt")), "old\n");
    assert!(!dir.join("notes.md").exists());
    assert_eq!(
        trash,
        [("a.txt".to_owned(), b"new\n".to_vec()), ("notes.md".to_owned(), b"notes\n".to_vec())]
    );
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert!(turns[0].undone && turns[0].started_undo.is_none(), "{turns:?}");
}

#[tokio::test]
async fn undo_skips_changed_after_turn() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "old\n");
    one_call_turn(&world, 1, 1, &dir, || write(&dir.join("a.txt"), "new\n")).await;
    write(&dir.join("a.txt"), "mine\n");

    let plan = plan(&world, 1, false, 1).await;
    assert_eq!(entry(&plan, "a.txt").action, PlanAction::Skip(UndoReason::ChangedAfterTurn));
    apply(&world, &plan).await;
    assert_eq!(read(&dir.join("a.txt")), "mine\n", "the user's edit stays");
}

#[tokio::test]
async fn undo_force_overrides() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "old\n");
    one_call_turn(&world, 1, 1, &dir, || write(&dir.join("a.txt"), "new\n")).await;
    write(&dir.join("a.txt"), "mine\n");

    let plan = plan(&world, 1, true, 1).await;
    let forced = entry(&plan, "a.txt");
    assert_eq!(forced.forced, Some(UndoReason::ChangedAfterTurn));
    assert_eq!(forced.expect, Expect::Blob(blob_id(b"mine\n")));
    let trash = apply(&world, &plan).await;
    assert_eq!(read(&dir.join("a.txt")), "old\n");
    assert_eq!(trash, [("a.txt".to_owned(), b"mine\n".to_vec())], "the user's bytes are kept");
}

#[tokio::test]
async fn undo_is_a_stack() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    for n in 1..=2 {
        one_call_turn(&world, 1, n, &dir, || write(&dir.join("a.txt"), &format!("{n}\n"))).await;
    }
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    let order: Vec<_> = turns.iter().map(|refs| refs.turn).collect();
    assert_eq!(order, [turn(2), turn(1)], "newest first");
    assert_eq!(stack_blocker(&turns, turn(1)), Some(turn(2)));
    assert_eq!(stack_blocker(&turns, turn(2)), None);

    let older = plan(&world, 1, false, 1).await;
    assert_eq!(
        entry(&older, "a.txt").action,
        PlanAction::Skip(UndoReason::LaterTurnNotUndone),
        "a later turn changed the path"
    );
    apply(&world, &plan(&world, 2, false, 2).await).await;
    assert_eq!(read(&dir.join("a.txt")), "1\n");
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert_eq!(stack_blocker(&turns, turn(1)), None, "turn 2 is undone");
    apply(&world, &plan(&world, 1, false, 3).await).await;
    assert_eq!(read(&dir.join("a.txt")), "0\n");
}

#[tokio::test]
async fn redo_after_undo() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "old\n");
    one_call_turn(&world, 1, 1, &dir, || {
        write(&dir.join("a.txt"), "new\n");
        write(&dir.join("b.txt"), "b\n");
    })
    .await;
    apply(&world, &plan(&world, 1, false, 1).await).await;
    assert!(!dir.join("b.txt").exists());

    let redo = world.snapshots.plan_redo(conversation(1), &[], false, limits()).await.unwrap();
    assert!(redo.redo && !redo.resumed);
    assert_eq!((redo.undo_id.as_str(), redo.turn), (undo_id(1).as_str(), turn(1)));
    assert_eq!(
        actions(&redo),
        [
            ("a.txt".to_owned(), PlanAction::Write { blob: blob_id(b"new\n"), mode: 0o644 }),
            ("b.txt".to_owned(), PlanAction::Write { blob: blob_id(b"b\n"), mode: 0o644 }),
        ]
    );
    apply(&world, &redo).await;
    assert_eq!(
        (read(&dir.join("a.txt")), read(&dir.join("b.txt"))),
        ("new\n".into(), "b\n".into())
    );
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert!(!turns[0].undone, "the redo took the undo back");
    let none = world.snapshots.plan_redo(conversation(1), &[], false, limits()).await;
    assert!(matches!(none, Err(efr_snapshot::SnapshotError::NoRedo)), "{none:?}");
    // The turn can be undone again.
    apply(&world, &plan(&world, 1, false, 2).await).await;
    assert_eq!(read(&dir.join("a.txt")), "old\n");
}

#[tokio::test]
async fn undo_leaves_left_out_paths_alone() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("grows.bin"), "small\n");
    one_call_turn(&world, 1, 1, &dir, || {
        write(&dir.join("grows.bin"), &"x".repeat(4096));
        write(&dir.join("big.bin"), &"y".repeat(4096));
    })
    .await;
    let plan = plan(&world, 1, true, 1).await;
    assert_eq!(
        entry(&plan, "grows.bin").action,
        PlanAction::NotCovered(LeftOutReason::TooLarge),
        "the last snapshot left the grown file out"
    );
    assert!(
        plan.roots[0].entries.iter().all(|entry| entry.path != "big.bin"),
        "a file that no snapshot took is in no tree: {plan:#?}"
    );
    apply(&world, &plan).await;
    assert_eq!(fs::read(dir.join("grows.bin")).unwrap().len(), 4096);
    assert!(dir.join("big.bin").exists());
}

#[tokio::test]
async fn undo_leaves_changes_of_another_conversation() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let first =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "a by 1\n");
    world.snapshots.after_call(first, limits()).await;
    // Another conversation's turn writes in the same root between two calls of turn 1.
    one_call_turn(&world, 2, 2, &dir, || write(&dir.join("b.txt"), "b by 2\n")).await;
    let second =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(second, limits()).await;
    world.snapshots.turn_end(turn(1), limits()).await;

    let plan = plan(&world, 1, true, 1).await;
    assert_eq!(entry(&plan, "b.txt").action, PlanAction::Skip(UndoReason::OutsideTurnCalls));
    apply(&world, &plan).await;
    assert_eq!(read(&dir.join("a.txt")), "a\n");
    assert_eq!(read(&dir.join("b.txt")), "b by 2\n", "the other conversation's file stays");
}

#[tokio::test]
async fn undo_leaves_user_edit_between_calls() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let first =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "by the call\n");
    world.snapshots.after_call(first, limits()).await;
    write(&dir.join("mine.txt"), "the user's\n");
    let second =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(second, limits()).await;
    world.snapshots.turn_end(turn(1), limits()).await;

    let plan = plan(&world, 1, false, 1).await;
    assert_eq!(entry(&plan, "mine.txt").action, PlanAction::Skip(UndoReason::OutsideTurnCalls));
    apply(&world, &plan).await;
    assert_eq!(read(&dir.join("a.txt")), "a\n");
    assert_eq!(read(&dir.join("mine.txt")), "the user's\n");
}

#[tokio::test]
async fn undo_plan_trashes_every_overwrite() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("changed.txt"), "1\n");
    write(&dir.join("deleted.txt"), "2\n");
    std::os::unix::fs::symlink("changed.txt", dir.join("link")).unwrap();
    one_call_turn(&world, 1, 1, &dir, || {
        write(&dir.join("changed.txt"), "one\n");
        fs::remove_file(dir.join("deleted.txt")).unwrap();
        write(&dir.join("new.txt"), "3\n");
        fs::remove_file(dir.join("link")).unwrap();
        std::os::unix::fs::symlink("new.txt", dir.join("link")).unwrap();
    })
    .await;
    let plan = plan(&world, 1, false, 1).await;
    for entry in plan.roots[0].entries.iter().filter(|entry| entry.acts()) {
        let over_something = entry.expect != Expect::Missing;
        assert_eq!(entry.trash.is_some(), over_something, "{entry:?}");
    }
    assert_eq!(entry(&plan, "deleted.txt").trash, None, "nothing is there to keep");
    assert_eq!(entry(&plan, "link").action, PlanAction::Link { target: b"changed.txt".to_vec() });
    let trash = apply(&world, &plan).await;
    let kept: Vec<&str> = trash.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(kept, ["changed.txt", "link", "new.txt"]);
    assert_eq!(fs::read_link(dir.join("link")).unwrap(), Path::new("changed.txt"));
    assert_eq!(read(&dir.join("deleted.txt")), "2\n");
}

#[tokio::test]
async fn undo_never_writes_an_unknown_expectation() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "old\n");
    one_call_turn(&world, 1, 1, &dir, || write(&dir.join("a.txt"), "new\n")).await;
    fs::remove_file(dir.join("a.txt")).unwrap();
    fs::create_dir(dir.join("a.txt")).unwrap();
    for force in [false, true] {
        let plan = plan(&world, 1, force, 1).await;
        let unknown = entry(&plan, "a.txt");
        assert_eq!(unknown.action, PlanAction::Skip(UndoReason::Unknown), "force {force}");
        assert_eq!(unknown.expect, Expect::Unknown);
    }
}

/// A turn of one call that efrd did not end: its store keeps `pre` and `last` and no
/// `post`, and a new store object stands for efrd after its restart.
async fn stopped_turn(world: &World, dir: &Path) -> Snapshots {
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(dir)], limits()).await;
    write(&dir.join("a.txt"), "by the call\n");
    world.snapshots.after_call(call, limits()).await;
    write(&dir.join("mine.txt"), "the user's, after the call\n");
    store(world.base.path(), &world.clock)
}

#[tokio::test]
async fn undo_without_post_needs_force() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let restarted = stopped_turn(&world, &dir).await;
    let turns = restarted.turns_with_refs(conversation(1)).await;
    assert!(!turns[0].has_post, "{turns:?}");
    let plan = restarted
        .plan_undo(conversation(1), turn(1), &[], false, &undo_id(1), limits())
        .await
        .unwrap();
    assert_eq!(entry(&plan, "a.txt").action, PlanAction::Skip(UndoReason::NoRecordAfterTurn));
    let forced = restarted
        .plan_undo(conversation(1), turn(1), &[], true, &undo_id(1), limits())
        .await
        .unwrap();
    let a = entry(&forced, "a.txt");
    assert_eq!(a.forced, Some(UndoReason::NoRecordAfterTurn));
    assert!(a.acts());
}

#[tokio::test]
async fn undo_without_post_acts_on_touched_paths_only() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let restarted = stopped_turn(&world, &dir).await;
    let plan = restarted
        .plan_undo(conversation(1), turn(1), &[], true, &undo_id(1), limits())
        .await
        .unwrap();
    let paths: Vec<&str> = plan.roots[0].entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["a.txt"], "the user's file is in no tree of the turn");
    restarted.capture_before_undo(&plan, limits()).await.unwrap();
    apply_root(&restarted, &plan.roots[0]).await;
    restarted.finish_undo(&plan, limits()).await.unwrap();
    assert_eq!(read(&dir.join("a.txt")), "a\n");
    assert_eq!(read(&dir.join("mine.txt")), "the user's, after the call\n");
}

#[tokio::test]
async fn plan_undo_dry_run_writes_no_ref() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "old\n");
    one_call_turn(&world, 1, 1, &dir, || write(&dir.join("a.txt"), "new\n")).await;
    let store = world.only_store();
    let refs_before = refs(&store, &home).await;
    let index = store.with_extension("index");
    let index_before = fs::read(&index).unwrap();

    let plan = plan(&world, 1, false, 1).await;
    assert!(entry(&plan, "a.txt").acts());
    assert_eq!(refs(&store, &home).await, refs_before, "a plan writes no ref");
    assert_eq!(fs::read(&index).unwrap(), index_before, "nor the index");
    assert_eq!(read(&dir.join("a.txt")), "new\n");
}

#[tokio::test]
async fn undo_after_a_stop_in_the_middle_can_return() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    write(&dir.join("b.txt"), "b\n");
    one_call_turn(&world, 1, 1, &dir, || {
        write(&dir.join("a.txt"), "a2\n");
        write(&dir.join("b.txt"), "b2\n");
    })
    .await;
    let first = plan(&world, 1, false, 1).await;
    world.snapshots.capture_before_undo(&first, limits()).await.unwrap();
    // The applier writes a.txt, then efrd stops.
    let mut half = first.roots[0].clone();
    half.entries.retain(|entry| entry.path == "a.txt");
    apply_root(&world.snapshots, &half).await;
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert_eq!(turns[0].started_undo.as_deref(), Some(undo_id(1).as_str()));
    assert!(!turns[0].undone);

    // A redo takes back the part that was done.
    let back = world.snapshots.plan_redo(conversation(1), &[], false, limits()).await.unwrap();
    assert!(back.redo && back.resumed);
    assert_eq!(
        entry(&back, "a.txt").action,
        PlanAction::Write { blob: blob_id(b"a2\n"), mode: 0o644 }
    );
    assert_eq!(entry(&back, "b.txt").action, PlanAction::Skip(UndoReason::AlreadyDone));

    // Or the next undo resumes it, under the same id.
    let again = plan(&world, 1, false, 2).await;
    assert!(again.resumed);
    assert_eq!(again.undo_id, undo_id(1));
    assert_eq!(entry(&again, "a.txt").action, PlanAction::Skip(UndoReason::AlreadyDone));
    assert!(entry(&again, "b.txt").acts());
    apply(&world, &again).await;
    assert_eq!((read(&dir.join("a.txt")), read(&dir.join("b.txt"))), ("a\n".into(), "b\n".into()));
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert!(turns[0].undone && turns[0].started_undo.is_none(), "{turns:?}");
}

#[tokio::test]
async fn undo_gives_a_new_ignored_file_a_private_mode() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join(".gitignore"), ".env\nlocal.cfg\n");
    write(&dir.join(".env"), "KEY=1\n");
    write(&dir.join("local.cfg"), "token\n");
    fs::set_permissions(dir.join(".env"), fs::Permissions::from_mode(0o640)).unwrap();
    write(&dir.join("plain.txt"), "plain\n");
    one_call_turn(&world, 1, 1, &dir, || {
        for name in [".env", "local.cfg", "plain.txt"] {
            fs::remove_file(dir.join(name)).unwrap();
        }
    })
    .await;
    let plan = plan(&world, 1, false, 1).await;
    assert_eq!(
        entry(&plan, ".env").action,
        PlanAction::Write { blob: blob_id(b"KEY=1\n"), mode: 0o640 },
        "the recorded mode"
    );
    assert_eq!(
        entry(&plan, "local.cfg").action,
        PlanAction::Write { blob: blob_id(b"token\n"), mode: 0o600 },
        "an ignored file with no recorded mode"
    );
    assert_eq!(
        entry(&plan, "plain.txt").action,
        PlanAction::Write { blob: blob_id(b"plain\n"), mode: 0o644 }
    );
}
