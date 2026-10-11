//! Whether the turn's snapshot covers what a destructive command can destroy.

use std::fs;

use efr_snapshot::{CoverResult, LeftOutReason, Limits, RootCover};
use pretty_assertions::assert_eq;

use crate::support::{
    World, conversation, git, git_root, limits, project, refs, root, turn, turn_ref, write,
};

fn path(dir: &std::path::Path, name: &str) -> RootCover {
    RootCover::Path(dir.join(name))
}

#[tokio::test]
async fn covers_reports_left_out_paths() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("data/small.txt"), "s\n");
    fs::write(dir.join("data/dump.sql"), vec![b'd'; 4096]).unwrap();
    write(&dir.join("src/a.rs"), "a\n");
    let asks = [
        path(&dir, "data"),
        path(&dir, "data/small.txt"),
        path(&dir, "src"),
        path(&dir, "gone"),
        RootCover::Path(dir.clone()),
        RootCover::Path(world.home()),
    ];
    let found =
        world.snapshots.covers(conversation(1), turn(1), &root(&dir), &asks, limits()).await;
    assert_eq!(
        found,
        [
            CoverResult::LeftOut(LeftOutReason::TooLarge),
            CoverResult::Covered,
            CoverResult::Covered,
            CoverResult::Covered,
            CoverResult::IsRoot,
            CoverResult::OutsideRoot,
        ]
    );
}

#[tokio::test]
async fn covers_is_false_for_an_ignored_file_in_a_skipped_dir() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join(".gitignore"), "target/\n*.log\n");
    write(&dir.join("target/debug/app"), "x\n");
    write(&dir.join("small.log"), "l\n");
    let asks = [path(&dir, "target"), path(&dir, "target/debug/app"), path(&dir, "small.log")];
    let found =
        world.snapshots.covers(conversation(1), turn(1), &root(&dir), &asks, limits()).await;
    let skipped = CoverResult::LeftOut(LeftOutReason::SkippedDir);
    assert_eq!(found, [skipped, skipped, CoverResult::Covered]);
}

#[tokio::test]
async fn covers_is_false_with_snapshot_ignored_none() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join(".gitignore"), ".env\n");
    write(&dir.join(".env"), "TOKEN=1\n");
    write(&dir.join("a.txt"), "a\n");
    let off = Limits { ignored_small: false, ..limits() };
    let asks = [path(&dir, ".env"), path(&dir, "a.txt")];
    let found = world.snapshots.covers(conversation(1), turn(1), &root(&dir), &asks, off).await;
    assert_eq!(found, [CoverResult::LeftOut(LeftOutReason::IgnoredOff), CoverResult::Covered]);
}

#[tokio::test]
async fn rm_in_git_dir_is_never_covered() {
    let world = World::new();
    let dir = project(&world, "p").await;
    let asks = [path(&dir, ".git"), path(&dir, ".git/hooks"), path(&dir, "sub/.git/config")];
    let found =
        world.snapshots.covers(conversation(1), turn(1), &git_root(&dir), &asks, limits()).await;
    assert_eq!(found, [CoverResult::GitInternal; 3]);
}

#[tokio::test]
async fn reset_hard_with_staged_only_content_is_not_covered() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "p").await;
    let asks = [RootCover::Tracked, RootCover::Refs];
    let found =
        world.snapshots.covers(conversation(1), turn(1), &git_root(&dir), &asks, limits()).await;
    assert_eq!(found, [CoverResult::Covered, CoverResult::Covered]);

    // Content that is only in the index: not in the work tree, not in HEAD.
    write(&dir.join("a.txt"), "staged\n");
    git(&dir, &home, &[], &["add", "a.txt"]).await;
    write(&dir.join("a.txt"), "a\n");
    let found =
        world.snapshots.covers(conversation(1), turn(1), &git_root(&dir), &asks, limits()).await;
    assert_eq!(found, [CoverResult::StagedOnly, CoverResult::Covered]);

    // A tracked file that the snapshot leaves out.
    git(&dir, &home, &[], &["reset", "-q"]).await;
    fs::write(dir.join("a.txt"), vec![b'x'; 4096]).unwrap();
    let found = world
        .snapshots
        .covers(conversation(1), turn(1), &git_root(&dir), &[RootCover::Tracked], limits())
        .await;
    assert_eq!(found, [CoverResult::LeftOut(LeftOutReason::TooLarge)]);

    // Without trusted git dirs nothing about the refs is known.
    let found = world
        .snapshots
        .covers(
            conversation(1),
            turn(2),
            &root(&dir),
            &asks,
            Limits { max_file_bytes: 8192, ..limits() },
        )
        .await;
    assert_eq!(found, [CoverResult::NoMirror, CoverResult::NoMirror]);
}

#[tokio::test]
async fn stash_clear_with_two_entries_is_not_covered() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "p").await;
    write(&dir.join("a.txt"), "one\n");
    git(&dir, &home, &[], &["stash", "-q"]).await;
    let found = world
        .snapshots
        .covers(conversation(1), turn(1), &git_root(&dir), &[RootCover::Stash], limits())
        .await;
    assert_eq!(found, [CoverResult::Covered], "one entry: the mirror holds refs/stash");

    write(&dir.join("a.txt"), "two\n");
    git(&dir, &home, &[], &["stash", "-q"]).await;
    let found = world
        .snapshots
        .covers(conversation(1), turn(2), &git_root(&dir), &[RootCover::Stash], limits())
        .await;
    assert_eq!(found, [CoverResult::StashEntries], "the older entry lives only in the reflog");
}

#[tokio::test]
async fn covers_takes_pre_when_missing() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "p").await;
    write(&dir.join("new.txt"), "n\n");
    let asks = [RootCover::Untracked, path(&dir, "new.txt")];
    let found =
        world.snapshots.covers(conversation(1), turn(1), &git_root(&dir), &asks, limits()).await;
    assert_eq!(found, [CoverResult::Covered, CoverResult::Covered]);
    let store = world.only_store();
    let names = refs(&store, &home).await;
    assert!(names.contains(&turn_ref(1, "pre")), "{names:?}");
    assert!(names.iter().any(|name| name.starts_with("refs/efr-mirror/")), "{names:?}");

    // An untracked file that the snapshot leaves out.
    fs::write(dir.join("dump.sql"), vec![b'd'; 4096]).unwrap();
    let found = world
        .snapshots
        .covers(conversation(1), turn(1), &git_root(&dir), &[RootCover::Untracked], limits())
        .await;
    assert_eq!(found, [CoverResult::LeftOut(LeftOutReason::TooLarge)]);
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert!(end.kept, "{end:?}");
}
