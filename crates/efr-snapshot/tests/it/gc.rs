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

/// Sets the time of every loose object of the store at `git_dir` far back, so `git
/// prune --expire=1.hour.ago` may remove what nothing reaches.
fn age_objects(git_dir: &std::path::Path) {
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
    let mut stack = vec![git_dir.join("objects")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                stack.push(path);
            } else {
                std::fs::File::open(&path).unwrap().set_modified(old).unwrap();
            }
        }
    }
}

#[tokio::test]
async fn a_collection_keeps_the_trees_of_a_running_turn_and_call() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    write(&dir.join("b.txt"), "only before the long turn\n");
    // A long turn of conversation 2: its first tree holds the old b.txt, and the index
    // moves past it.
    let long =
        world.snapshots.before_call(conversation(2), turn(10), vec![root(&dir)], limits()).await;
    write(&dir.join("b.txt"), "changed\n");
    world.snapshots.after_call(long, limits()).await.unwrap();
    // A long call of the same turn, whose tree before still holds c.txt.
    write(&dir.join("c.txt"), "only before the long call\n");
    let call =
        world.snapshots.before_call(conversation(2), turn(10), vec![root(&dir)], limits()).await;
    // Conversation 1 has more turns than the collector keeps, so it prunes.
    for n in 1..=2 {
        let short =
            world.snapshots.before_call(conversation(1), turn(n), vec![root(&dir)], limits()).await;
        std::fs::remove_file(dir.join("c.txt")).ok();
        write(&dir.join("a.txt"), &format!("{n}\n"));
        world.snapshots.after_call(short, limits()).await;
        world.snapshots.finish_turn(turn(n), limits()).await.unwrap();
    }
    age_objects(&world.only_store());

    let report = world.snapshots.gc(1, KEEP_LONG).await;
    assert_eq!(report.refs_deleted, 2);

    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    let paths: Vec<&str> = changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "c.txt"], "the call's tree before is still there");
    let turn = world.snapshots.finish_turn(turn(10), limits()).await.unwrap();
    let paths: Vec<&str> = turn.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "b.txt"], "the turn's first tree is still there");
    let diff = world.snapshots.turn_diff(conversation(2), None, true, 100).await.unwrap().unwrap();
    assert!(diff.diff.unwrap().contains("-only before the long turn\n"));
}
