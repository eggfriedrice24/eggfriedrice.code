//! The collector: the newest turns of each conversation stay, idle stores go.

use std::time::Duration;

use pretty_assertions::assert_eq;

use crate::support::{World, conversation, limits, root, stores, turn, write};

const KEEP_LONG: Duration = Duration::from_secs(30 * 24 * 3600);

#[tokio::test]
async fn only_the_newest_turns_keep_their_refs_and_snapshots_still_work() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    for n in 1..=3 {
        let call =
            world.snapshots.before_call(conversation(1), turn(n), vec![root(&dir)], limits()).await;
        write(&dir.join("a.txt"), &format!("{n}\n"));
        world.snapshots.after_call(call, limits()).await;
        world.snapshots.finish_turn(turn(n), limits()).await.unwrap();
    }

    let report = world.snapshots.gc(1, KEEP_LONG).await;
    assert_eq!((report.refs_deleted, report.stores_deleted), (4, 0));
    let old = world.snapshots.turn_diff(conversation(1), Some(turn(1)), false, 100).await;
    assert!(old.unwrap().unwrap().changes.is_empty(), "turn 1 has no refs left");
    let kept = world.snapshots.turn_diff(conversation(1), None, true, 100).await.unwrap().unwrap();
    assert_eq!(kept.turn_id, turn(3));
    assert!(kept.diff.unwrap().contains("-2\n+3\n"));

    // NOTE: the index still names its objects after the prune, so the next snapshot
    // writes a tree.
    let call =
        world.snapshots.before_call(conversation(1), turn(4), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "4\n");
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    assert_eq!(changes.files[0].path, "a.txt");
}

#[tokio::test]
async fn a_store_without_a_snapshot_for_too_long_is_deleted() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await;
    assert_eq!(stores(&world.store_dir()).len(), 1);

    assert_eq!(world.snapshots.gc(50, KEEP_LONG).await.stores_deleted, 0);
    world.clock.advance(KEEP_LONG + Duration::from_secs(1));
    assert_eq!(world.snapshots.gc(50, KEEP_LONG).await.stores_deleted, 1);
    assert_eq!(stores(&world.store_dir()), Vec::<std::path::PathBuf>::new());

    // A new snapshot makes the store again.
    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "1\n");
    assert!(world.snapshots.after_call(call, limits()).await.is_some());
}
