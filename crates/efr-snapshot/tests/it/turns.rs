//! A turn's first and last snapshots, its refs and its diff.

use efr_protocol::ChangeKind;
use pretty_assertions::assert_eq;

use crate::support::{World, conversation, git, limits, root, tree_of, turn, write};

#[tokio::test]
async fn a_turn_compares_its_first_snapshot_with_its_last_across_calls_and_writes() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "1\n2\n3\n");

    let first =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "1\ntwo\n3\n");
    world.snapshots.after_call(first, limits()).await.unwrap();
    // A file tool's write: the turn already has its first snapshot of this root.
    world.snapshots.before_write(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("b.txt"), "new\n");

    let changes = world.snapshots.finish_turn(turn(1), limits()).await.unwrap();
    let files: Vec<(&str, ChangeKind, u32, u32)> = changes
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.kind, file.added, file.removed))
        .collect();
    assert_eq!(files, [("a.txt", ChangeKind::Modified, 1, 1), ("b.txt", ChangeKind::Added, 1, 0)]);
    assert_eq!((changes.added, changes.removed), (2, 1));
}

#[tokio::test]
async fn a_turn_without_a_snapshot_has_no_changes() {
    let world = World::new();
    assert_eq!(world.snapshots.finish_turn(turn(9), limits()).await, None);
}

#[tokio::test]
async fn the_diff_of_a_turn_comes_back_with_and_without_the_patch() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "old\n");
    for n in 1..=2 {
        let call =
            world.snapshots.before_call(conversation(1), turn(n), vec![root(&dir)], limits()).await;
        write(&dir.join("a.txt"), &format!("turn {n}\n"));
        world.snapshots.after_call(call, limits()).await;
        world.snapshots.finish_turn(turn(n), limits()).await.unwrap();
    }

    let newest = world.snapshots.turn_diff(conversation(1), None, true, 20_000).await.unwrap();
    let newest = newest.unwrap();
    assert_eq!(newest.turn_id, turn(2));
    assert_eq!(newest.changes.files[0].path, "a.txt");
    let diff = newest.diff.unwrap();
    assert!(diff.contains("--- a/a.txt\n+++ b/a.txt\n"), "{diff}");
    assert!(diff.contains("-turn 1\n+turn 2\n"), "{diff}");

    let first = world.snapshots.turn_diff(conversation(1), Some(turn(1)), false, 20_000).await;
    let first = first.unwrap().unwrap();
    assert_eq!(first.turn_id, turn(1));
    assert_eq!(first.diff, None, "stat leaves the patch out");
    assert_eq!((first.changes.added, first.changes.removed), (1, 1));

    let cut = world.snapshots.turn_diff(conversation(1), None, true, 2).await.unwrap().unwrap();
    assert!(cut.diff.unwrap().ends_with(" more lines\n"));

    let none = world.snapshots.turn_diff(conversation(7), None, true, 20_000).await.unwrap();
    assert_eq!(none, None, "a conversation without snapshots has no turn to show");
}

#[tokio::test]
async fn a_scratch_turn_diff_shows_the_scratch_prefix() {
    let world = World::new();
    let scratch = world.dir("scratch");
    let roots = vec![efr_snapshot::Root::new(&scratch, "$SCRATCH/")];
    world.snapshots.before_call(conversation(1), turn(1), roots, limits()).await;
    write(&scratch.join("plot.py"), "print(1)\n");
    world.snapshots.finish_turn(turn(1), limits()).await.unwrap();
    let diff = world.snapshots.turn_diff(conversation(1), None, true, 100).await.unwrap().unwrap();
    assert_eq!(diff.changes.files[0].path, "$SCRATCH/plot.py");
    assert!(diff.diff.unwrap().contains("+++ b/$SCRATCH/plot.py\n"));
}

#[tokio::test]
async fn a_file_of_two_nested_roots_shows_once_in_the_diff() {
    let world = World::new();
    let outer = world.dir("p");
    let inner = world.dir("p/sub");
    write(&outer.join("a.txt"), "a\n");
    write(&inner.join("x.txt"), "old\n");
    let roots =
        vec![efr_snapshot::Root::new(&outer, ""), efr_snapshot::Root::new(&inner, "~/p/sub/")];
    let call = world.snapshots.before_call(conversation(1), turn(1), roots, limits()).await;
    write(&outer.join("a.txt"), "b\n");
    write(&inner.join("x.txt"), "new\n");
    world.snapshots.after_call(call, limits()).await.unwrap();
    world.snapshots.finish_turn(turn(1), limits()).await.unwrap();

    let found = world.snapshots.turn_diff(conversation(1), None, true, 100).await.unwrap().unwrap();
    let paths: Vec<&str> = found.changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "~/p/sub/x.txt"]);
    let diff = found.diff.unwrap();
    assert_eq!(diff.matches("diff --git ").count(), 2, "{diff}");
    assert_eq!(diff.matches("+new\n").count(), 1, "{diff}");
}

#[tokio::test]
async fn the_diff_of_a_turn_lists_an_ignored_file_without_its_content() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join(".gitignore"), ".env\nlocal/\n");
    write(&dir.join(".env"), "TOKEN=old-secret\n");
    write(&dir.join("local/key"), "KEY=old\n");
    write(&dir.join("a.txt"), "a\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join(".env"), "TOKEN=new-secret\n");
    write(&dir.join("local/key"), "KEY=new\n");
    write(&dir.join("a.txt"), "b\n");
    world.snapshots.after_call(call, limits()).await.unwrap();
    world.snapshots.finish_turn(turn(1), limits()).await.unwrap();

    let found = world.snapshots.turn_diff(conversation(1), None, true, 100).await.unwrap().unwrap();
    let paths: Vec<&str> = found.changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, [".env", "a.txt", "local/key"], "the list keeps the ignored files");
    let diff = found.diff.unwrap();
    assert!(!diff.contains("secret") && !diff.contains("KEY="), "{diff}");
    assert!(diff.contains("+b\n"), "{diff}");
    assert!(diff.contains(".env: ignored file, content not shown\n"), "{diff}");
    assert!(diff.contains("local/key: ignored file, content not shown\n"), "{diff}");
}

#[tokio::test]
async fn the_first_call_of_a_turn_in_an_unchanged_root_runs_only_the_two_listings() {
    let world = World::with_git_log();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join(".gitignore"), "vendor/\n.env\n");
    write(&dir.join("a.txt"), "a\n");
    write(&dir.join(".env"), "TOKEN=1\n");
    write(&dir.join("vendor/notes.md"), "n\n");
    write(&dir.join("vendor/lib/src/a.rs"), "fn a() {}\n");
    git(&dir.join("vendor/lib"), &home, &[], &["init", "-q"]).await;
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await;
    world.git_runs();

    // NOTE: vendor/ now has a file in the index, so git lists the ignored files below
    // it one by one, also those of the nested repository, which git add refuses.
    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    assert_eq!(world.git_runs(), ["ls-files", "ls-files"], "no add and no new tree");
    world.snapshots.after_call(call, limits()).await;
    assert_eq!(world.git_runs(), ["ls-files"], "a later snapshot skips the ignored files");
    world.snapshots.finish_turn(turn(2), limits()).await;
    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(2));
    let tree = tree_of(&world.only_store(), &home, &pre).await;
    let names: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(names, [".env", ".gitignore", "a.txt", "vendor/notes.md"]);
}
