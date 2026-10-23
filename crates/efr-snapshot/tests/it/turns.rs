//! A turn's first and last snapshots, its refs and its diff.

use std::fs;
use std::os::unix::fs::MetadataExt as _;

use efr_protocol::ChangeKind;
use efr_snapshot::{NOT_COVERED_FAILED, NOT_COVERED_STORE_CHANGED};
use pretty_assertions::assert_eq;

use crate::support::{
    World, blobs_of, conversation, git, limits, meta_of, refs, root, tree_of, turn, turn_ref, write,
};

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
    world.snapshots.before_call(conversation(1), turn(1), roots, limits()).await.discard();
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
async fn an_ignored_file_made_between_two_turns_is_in_the_first_snapshot_of_the_next() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join(".gitignore"), ".env*\nlocal/\n");
    write(&dir.join("local/key"), "KEY=1\n");
    write(&dir.join("a.txt"), "a\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "b\n");
    world.snapshots.after_call(call, limits()).await.unwrap();
    world.snapshots.finish_turn(turn(1), limits()).await.unwrap();

    // NOTE: no file of the index and no ignore rule changes; only the scan sees these.
    write(&dir.join(".env.local"), "TOKEN=1\n");
    write(&dir.join("local/other"), "KEY=2\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    fs::remove_file(dir.join("local/other")).unwrap();
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    let paths: Vec<&str> = changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["local/other"], "the call deleted a file that its snapshot took");
    let changes = world.snapshots.finish_turn(turn(2), limits()).await.unwrap();
    let paths: Vec<&str> = changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["local/other"], "the turn did not make .env.local");

    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(2));
    let tree = tree_of(&world.only_store(), &home, &pre).await;
    let names: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(names, [".env.local", ".gitignore", "a.txt", "local/key", "local/other"]);
}

#[tokio::test]
async fn the_first_call_of_a_turn_in_an_unchanged_root_runs_the_two_listings_and_keeps_pre() {
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
    assert_eq!(
        world.git_runs(),
        ["commit-tree", "ls-files", "ls-files", "update-index", "update-ref"],
        "the refresh, no add and no new tree; the commit and the ref of pre"
    );
    world.snapshots.after_call(call, limits()).await;
    assert_eq!(world.git_runs(), ["ls-files"], "a later snapshot skips the ignored files");
    world.snapshots.finish_turn(turn(2), limits()).await;
    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(2));
    let tree = tree_of(&world.only_store(), &home, &pre).await;
    let names: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(names, [".env", ".gitignore", "a.txt", "vendor/notes.md"]);
}

#[tokio::test]
async fn a_new_ignore_rule_takes_effect_at_the_next_turn() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join(".gitignore"), "local/\n");
    write(&dir.join("a.txt"), "a\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    assert_eq!(world.snapshots.finish_turn(turn(1), limits()).await, None);

    // NOTE: a file above the limit of untracked files, which only the ignored scan
    // takes; first under a rule that does not name it, then under one that does.
    fs::write(dir.join("dump.sql"), vec![b'x'; 4096]).unwrap();
    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(2), limits()).await;
    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(2));
    let tree = tree_of(&world.only_store(), &home, &pre).await;
    assert!(!tree.contains_key("dump.sql"), "a large untracked file stays out");

    write(&dir.join(".gitignore"), "local/\n*.sql\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(3), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(3), limits()).await;
    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(3));
    let tree = tree_of(&world.only_store(), &home, &pre).await;
    assert!(tree.contains_key("dump.sql"), "a small ignored file comes in: {tree:?}");
}

#[tokio::test]
async fn snapshot_pre_ref_written_at_first_capture() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let roots = vec![root(&dir).with_mode("auto")];
    let call = world.snapshots.before_call(conversation(1), turn(1), roots, limits()).await;
    assert!(!call.is_empty());
    let store = world.only_store();
    assert_eq!(
        refs(&store, &home).await,
        [turn_ref(1, "pre")],
        "pre is there before the turn ends"
    );
    let meta = meta_of(&store, &home, &turn_ref(1, "pre")).await;
    assert_eq!(meta["phase"], "pre");
    assert_eq!(meta["mode"], "auto");
    let metadata = fs::metadata(&dir).unwrap();
    assert_eq!(meta["dev_ino"], serde_json::json!([metadata.dev(), metadata.ino()]));

    // A file tool's write in the same turn finds the turn's pre and writes no second.
    world.snapshots.before_write(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    assert_eq!(refs(&store, &home).await, [turn_ref(1, "pre")]);
}

#[tokio::test]
async fn failed_first_capture_marks_root_not_covered() {
    let world = World::new();
    let marker = world.base.path().join("fail-listings");
    let script = world.base.path().join("git-failing");
    let text = format!(
        "#!/bin/sh\n\
         for arg; do\n\
         \x20 if [ \"$arg\" = ls-files ] && [ -e '{}' ]; then exit 1; fi\n\
         done\n\
         exec git \"$@\"\n",
        marker.display()
    );
    let world = world.with_git_script(&script, &text);
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    fs::write(&marker, "").unwrap();
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    assert!(call.is_empty(), "the first snapshot failed");
    fs::remove_file(&marker).unwrap();

    // The next snapshot works, but a call of the turn may have written in between: it
    // never becomes pre.
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "b\n");
    assert!(world.snapshots.after_call(call, limits()).await.is_some());
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(end.not_covered, [(dir.clone(), NOT_COVERED_FAILED.to_owned())]);
    assert!(!end.kept && end.snapshots.is_empty() && end.changes.is_none());
    assert_eq!(refs(&world.only_store(), &home).await, Vec::<String>::new());
}

#[tokio::test]
async fn turn_diff_skips_a_turn_without_post() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "b\n");
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await.unwrap();
    // NOTE: turn 2 has its pre, and efrd stops before its end.
    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "c\n");
    world.snapshots.after_call(call, limits()).await;

    let newest = world.snapshots.turn_diff(conversation(1), None, false, 100).await.unwrap();
    assert_eq!(newest.unwrap().turn_id, turn(1), "the newest turn with a post");
    let stopped = world.snapshots.turn_diff(conversation(1), Some(turn(2)), false, 100).await;
    assert!(stopped.unwrap().unwrap().changes.is_empty(), "no record after the turn");
}

#[tokio::test]
async fn after_call_moves_last_and_records_touched() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        write(&dir.join(name), "0\n");
    }
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "1\n");
    world.snapshots.after_call(call, limits()).await.unwrap();
    let store = world.only_store();
    let last = meta_of(&store, &home, &turn_ref(1, "last")).await;
    assert_eq!(last["phase"], "last");
    assert_eq!(last["touched"], serde_json::json!(["a.txt"]));
    let tree = blobs_of(&store, &home, &turn_ref(1, "last")).await;
    let pre = blobs_of(&store, &home, &turn_ref(1, "pre")).await;
    assert_ne!(tree["a.txt"], pre["a.txt"], "last holds the call's write");

    // The user writes between two calls; a quiet call moves nothing.
    write(&dir.join("b.txt"), "user\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    assert_eq!(world.snapshots.after_call(call, limits()).await, None);
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    fs::rename(dir.join("c.txt"), dir.join("e.txt")).unwrap();
    world.snapshots.after_call(call, limits()).await.unwrap();
    let last = meta_of(&store, &home, &turn_ref(1, "last")).await;
    assert_eq!(last["touched"], serde_json::json!(["a.txt", "c.txt", "e.txt"]));
    // A file tool writes d.txt.
    write(&dir.join("d.txt"), "tool\n");
    world.snapshots.after_write(turn(1), vec![dir.join("d.txt")], limits()).await;
    let last = meta_of(&store, &home, &turn_ref(1, "last")).await;
    assert_eq!(last["touched"], serde_json::json!(["a.txt", "c.txt", "d.txt", "e.txt"]));
    let tree = blobs_of(&store, &home, &turn_ref(1, "last")).await;
    assert_ne!(tree["d.txt"], pre["d.txt"], "last holds the file tool's write");

    let end = world.snapshots.turn_end(turn(1), limits()).await;
    let changed: Vec<&str> =
        end.changes.as_ref().unwrap().files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(changed, ["a.txt", "b.txt", "d.txt", "e.txt"], "the turn lists every change");
    let post = meta_of(&store, &home, &turn_ref(1, "post")).await;
    assert_eq!(
        post["touched"],
        serde_json::json!(["a.txt", "c.txt", "d.txt", "e.txt"]),
        "the user's write between two calls is not the turn's"
    );
    assert_eq!(post["touched_more"], false);
    assert_eq!(
        refs(&store, &home).await,
        [turn_ref(1, "post"), turn_ref(1, "pre")],
        "post replaces last"
    );
}

#[tokio::test]
async fn turn_end_reports_a_deleted_pre_ref() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "b\n");
    world.snapshots.after_call(call, limits()).await;
    // NOTE: an approved command outside the sandbox runs with the user's rights and can
    // change efr's store.
    let store = world.only_store();
    let name = turn_ref(1, "pre");
    git(&home, &home, &[("GIT_DIR", store.as_path())], &["update-ref", "-d", &name]).await;

    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(end.not_covered, [(dir.clone(), NOT_COVERED_STORE_CHANGED.to_owned())]);
    assert!(!end.kept);
    assert!(end.changes.is_some(), "the list of changes still comes");
    assert!(!refs(&store, &home).await.contains(&turn_ref(1, "post")), "no post on a lost pre");
}
