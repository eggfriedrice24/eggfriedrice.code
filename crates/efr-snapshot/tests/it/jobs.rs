//! Background jobs (efr's auto spec, section 3.18): the job outputs in `$SCRATCH/jobs`
//! stay out of every snapshot, a job's files join the turn that started it at the
//! job's end without the paths of a later turn, and the collector keeps the turn of a
//! job that still runs.

use std::time::Duration;

use efr_protocol::UndoReason;
use efr_snapshot::{CallSnapshot, PlanAction, Root, RootPlan};
use efr_stdx::time::Clock as _;
use pretty_assertions::assert_eq;

use crate::support::{
    World, conversation, limits, refs, root, store, tree_of, turn, turn_ref, write,
};
use crate::undo::undo_id;

/// The root of `$SCRATCH`, as efrd names it: its `jobs` dir stays out.
fn scratch(path: &std::path::Path) -> Root {
    Root::new(path, "$SCRATCH/").with_skip(["jobs"])
}

/// The paths of a plan's root with what undo does with each.
fn actions(root: &RootPlan) -> Vec<(String, PlanAction)> {
    root.entries.iter().map(|entry| (entry.path.clone(), entry.action.clone())).collect()
}

#[tokio::test]
async fn snapshot_skips_job_output() {
    let world = World::new();
    let dir = world.dir("scratch");
    write(&dir.join("notes.txt"), "a\n");
    write(&dir.join("jobs/j1.out"), "old output\n");
    // NOTE: a store that took the outputs before efr left them out: the first snapshot
    // of a new process drops them from its index.
    let older = store(world.base.path(), &world.clock);
    let call = older.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    older.after_call(call, limits()).await;
    older.turn_end(turn(1), limits()).await;
    let git_dir = world.only_store();
    let home = world.home();
    assert!(tree_of(&git_dir, &home, &turn_ref(1, "post")).await.contains_key("jobs/j1.out"));

    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![scratch(&dir)], limits()).await;
    write(&dir.join("notes.txt"), "b\n");
    write(&dir.join("jobs/j1.out"), "more output\n");
    write(&dir.join("jobs/j2.out"), "new output\n");
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    let paths: Vec<&str> = changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["$SCRATCH/notes.txt"]);
    world.snapshots.turn_end(turn(2), limits()).await;
    for phase in ["pre", "post"] {
        let tree = tree_of(&git_dir, &home, &turn_ref(2, phase)).await;
        assert_eq!(tree.keys().collect::<Vec<_>>(), ["notes.txt"], "{phase}");
    }
}

/// The start call of a job in turn `n` of `dir`, as efrd runs it: the snapshot before,
/// the snapshot after that knows the job runs on, and the snapshot of the job's roots
/// right after the start, which the job's end compares with.
async fn start_job(world: &World, n: u128, dir: &std::path::Path) -> CallSnapshot {
    let call =
        world.snapshots.before_call(conversation(1), turn(n), vec![root(dir)], limits()).await;
    world.snapshots.after_call(call.runs_on(true), limits()).await;
    world.snapshots.before_call(conversation(1), turn(n), vec![root(dir)], limits()).await
}

#[tokio::test]
async fn job_writes_join_start_turn_at_end() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let since = world.clock.now();
    let kept = start_job(&world, 1, &dir).await;
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert!(end.changes.is_none(), "{end:?}");
    world.clock.advance(Duration::from_secs(30));
    write(&dir.join("job.txt"), "hi\n");
    write(&dir.join("a.txt"), "changed by the job\n");

    let joined = world.snapshots.after_job(kept, since, limits()).await.unwrap();
    let paths: Vec<&str> = joined.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "job.txt"]);
    let git_dir = world.only_store();
    let names = refs(&git_dir, &world.home()).await;
    assert!(names.contains(&turn_ref(1, "last")), "{names:?}");
    assert!(!names.contains(&turn_ref(1, "post")), "{names:?}");
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert!(!turns[0].has_post, "the job's end left the turn with no record after it");

    // NOTE: the turn has no record after it now, so its undo needs force.
    let plan = world
        .snapshots
        .plan_undo(conversation(1), turn(1), &[], false, &undo_id(1), limits())
        .await
        .unwrap();
    assert_eq!(
        actions(&plan.roots[0]),
        [
            ("a.txt".to_owned(), PlanAction::Skip(UndoReason::NoRecordAfterTurn)),
            ("job.txt".to_owned(), PlanAction::Skip(UndoReason::NoRecordAfterTurn)),
        ]
    );
    let forced = world
        .snapshots
        .plan_undo(conversation(1), turn(1), &[], true, &undo_id(1), limits())
        .await
        .unwrap();
    let forced = actions(&forced.roots[0]);
    assert!(matches!(forced[0].1, PlanAction::Write { .. }), "{forced:?}");
    assert_eq!(forced[1], ("job.txt".to_owned(), PlanAction::Delete));
}

#[tokio::test]
async fn job_end_leaves_later_turn_paths_alone() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    write(&dir.join("later.txt"), "before\n");
    let since = world.clock.now();
    let kept = start_job(&world, 1, &dir).await;
    world.snapshots.turn_end(turn(1), limits()).await;
    world.clock.advance(Duration::from_secs(30));
    // The next turn writes one path in the same root while the job runs.
    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    write(&dir.join("later.txt"), "the later turn's\n");
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.turn_end(turn(2), limits()).await;
    world.clock.advance(Duration::from_secs(30));
    write(&dir.join("job.txt"), "hi\n");

    let joined = world.snapshots.after_job(kept, since, limits()).await.unwrap();
    let paths: Vec<&str> = joined.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["job.txt"]);
    let plan = world
        .snapshots
        .plan_undo(conversation(1), turn(1), &[], true, &undo_id(1), limits())
        .await
        .unwrap();
    assert_eq!(actions(&plan.roots[0]), [("job.txt".to_owned(), PlanAction::Delete)]);
    let git_dir = world.only_store();
    let last = tree_of(&git_dir, &world.home(), &turn_ref(1, "last")).await;
    assert!(last.contains_key("job.txt") && last.contains_key("later.txt"), "{last:?}");
}

/// A job that changed nothing leaves its turn's record as it was.
#[tokio::test]
async fn a_job_that_changed_nothing_keeps_the_record_after_its_turn() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let since = world.clock.now();
    let kept = start_job(&world, 1, &dir).await;
    world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(world.snapshots.after_job(kept, since, limits()).await, None);
    let names = refs(&world.only_store(), &world.home()).await;
    assert!(names.contains(&turn_ref(1, "post")), "{names:?}");
}

#[tokio::test]
async fn gc_keeps_start_turn_of_running_job() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    for n in 1..=3 {
        let call =
            world.snapshots.before_call(conversation(1), turn(n), vec![root(&dir)], limits()).await;
        write(&dir.join("a.txt"), &format!("{n}\n"));
        world.snapshots.after_call(call, limits()).await;
        world.snapshots.turn_end(turn(n), limits()).await;
    }
    let keep = [(conversation(1), turn(1))];
    let long = Duration::from_secs(30 * 24 * 3600);
    let report = world.snapshots.gc_keeping(1, long, 1 << 30, &keep).await;
    assert_eq!(report.refs_deleted, 2, "only turn 2 went");
    let names = refs(&world.only_store(), &world.home()).await;
    assert!(names.contains(&turn_ref(1, "pre")), "{names:?}");
    assert!(!names.contains(&turn_ref(2, "pre")), "{names:?}");
    assert!(names.contains(&turn_ref(3, "post")), "{names:?}");
}
