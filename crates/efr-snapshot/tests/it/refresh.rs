//! The stat data of a store's index: the refresh at the turn's first snapshot of a
//! root (the plan's W3-E) changes no tree, a racily clean file that really changes is
//! still found, and the listing of a large index (`diff-files` and `ls-files -o`) runs
//! no code of the project or of a nested repository.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::{Duration, SystemTime};

use efr_protocol::{ChangeKind, FileChange};
use efr_snapshot::{CoverResult, RootCover};
use pretty_assertions::assert_eq;

use crate::support::{
    World, conversation, git, git_root, index_blobs, limits, project, root, tree_of, turn,
    turn_ref, write,
};

/// The files below a root that make its index large enough for the split listing.
const MANY: usize = 1_100;

fn modified(path: &str) -> FileChange {
    FileChange {
        path: path.to_owned(),
        kind: ChangeKind::Modified,
        from: None,
        added: 1,
        removed: 1,
        binary: false,
    }
}

/// Writes `count` small files below `dir/fill`.
fn fill(dir: &Path, count: usize) {
    for n in 0..count {
        write(&dir.join(format!("fill/d{}/f{n}.txt", n % 10)), &format!("fill {n}\n"));
    }
}

fn set_time(path: &Path, time: SystemTime) {
    fs::File::options().write(true).open(path).unwrap().set_modified(time).unwrap();
}

#[tokio::test]
async fn the_turns_first_snapshot_refreshes_the_index_and_keeps_its_tree() {
    for count in [0, MANY] {
        let world = World::with_git_log();
        let home = world.home();
        let dir = world.dir("refreshed");
        write(&dir.join("a.txt"), "a\n");
        write(&dir.join("b.txt"), "b\n");
        fill(&dir, count);
        let call =
            world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
        write(&dir.join("a.txt"), "a2\n");
        // NOTE: the same bytes again, so only the stat data of `b.txt` is new.
        write(&dir.join("b.txt"), "b\n");
        assert!(world.snapshots.after_call(call, limits()).await.is_some());
        write(&dir.join("b.txt"), "b\n");
        assert!(world.snapshots.finish_turn(turn(1), limits()).await.is_some());
        let store = world.only_store();
        let index = index_blobs(&store, &home).await;
        let post = tree_of(&store, &home, &turn_ref(1, "post")).await;
        world.git_runs();

        let call =
            world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
        let runs = world.git_runs();
        let refreshes = runs.iter().filter(|run| *run == "update-index").count();
        assert_eq!(refreshes, 1, "{count} files: {runs:?}");
        assert_eq!(world.snapshots.after_call(call, limits()).await, None, "{count} files");
        let call =
            world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
        assert_eq!(world.snapshots.after_call(call, limits()).await, None, "{count} files");
        let runs = world.git_runs();
        assert!(!runs.contains(&"update-index".to_owned()), "{count} files: {runs:?}");
        assert_eq!(index_blobs(&store, &home).await, index, "{count} files: no blob changed");
        let pre = tree_of(&store, &home, &turn_ref(2, "pre")).await;
        assert_eq!(pre, post, "{count} files: the tree is the same");
    }
}

/// A file with a time in the future (2096) is racily clean at each snapshot: its stat
/// data says nothing about its content, so git must compare that.
#[tokio::test]
async fn a_racily_clean_file_is_still_found_when_it_really_changes() {
    for count in [0, MANY] {
        let world = World::new();
        let dir = world.dir("racy");
        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000_000);
        write(&dir.join("a.txt"), "one\n");
        write(&dir.join("b.txt"), "two\n");
        fill(&dir, count);
        set_time(&dir.join("a.txt"), later);
        set_time(&dir.join("b.txt"), later);
        let call =
            world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
        assert_eq!(world.snapshots.after_call(call, limits()).await, None, "{count} files");
        world.snapshots.finish_turn(turn(1), limits()).await;

        // The turn's first snapshot refreshes the index; then the same size and time.
        let call =
            world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
        write(&dir.join("a.txt"), "uno\n");
        set_time(&dir.join("a.txt"), later);
        let changes = world.snapshots.after_call(call, limits()).await.unwrap();
        assert_eq!(changes.files, [modified("a.txt")], "{count} files");

        let call =
            world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
        assert_eq!(world.snapshots.after_call(call, limits()).await, None, "{count} files");
        let call =
            world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
        write(&dir.join("b.txt"), "dos\n");
        set_time(&dir.join("b.txt"), later);
        let changes = world.snapshots.after_call(call, limits()).await.unwrap();
        assert_eq!(changes.files, [modified("b.txt")], "{count} files");
        let turn_changes = world.snapshots.finish_turn(turn(2), limits()).await.unwrap();
        assert_eq!(turn_changes.files, [modified("a.txt"), modified("b.txt")], "{count} files");
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

/// Plain git in the nested repository `dir`, with its fsmonitor off.
async fn nested_git(dir: &Path, home: &Path, args: &[&str]) {
    let all = [&["-c", "core.fsmonitor=false"], args].concat();
    git(dir, home, &[], &all).await;
}

#[tokio::test]
async fn the_listing_of_a_large_index_runs_no_code_of_the_project_or_a_nested_repository() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "trapped").await;
    let markers = world.base.path().join("markers");
    fs::create_dir_all(&markers).unwrap();
    let mark = |name: &str| format!("touch '{}/{name}'; true", markers.display());
    for (key, value) in [
        ("core.fsmonitor", mark("fsmonitor")),
        ("filter.x.clean", format!("{}; cat", mark("clean"))),
        ("diff.external", mark("external")),
    ] {
        git(&dir, &home, &[], &["config", key, &value]).await;
    }
    write(&dir.join(".gitattributes"), "* filter=x diff=x\n");
    write(&dir.join("sub"), "a file first\n");
    fill(&dir, MANY);
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![git_root(&dir)], limits()).await;
    // NOTE: a tracked file that becomes a nested repository enters the index as a
    // commit of it, so the listings compare that repository's `HEAD`.
    fs::remove_file(dir.join("sub")).unwrap();
    let sub = dir.join("sub");
    fs::create_dir_all(&sub).unwrap();
    nested_git(&sub, &home, &["init", "-q"]).await;
    write(&sub.join("n.txt"), "n\n");
    nested_git(&sub, &home, &["add", "n.txt"]).await;
    nested_git(&sub, &home, &["commit", "-q", "-m", "one"]).await;
    git(&sub, &home, &[], &["config", "core.fsmonitor", &mark("nested-fsmonitor")]).await;
    let clean = format!("{}; cat", mark("nested-clean"));
    git(&sub, &home, &[], &["config", "filter.x.clean", &clean]).await;
    write(&sub.join(".gitattributes"), "* filter=x\n");
    let hook = sub.join(".git/hooks/post-index-change");
    write(&hook, &format!("#!/bin/sh\n{}\n", mark("nested-hook")));
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    write(&dir.join("a.txt"), "changed\n");
    assert!(world.snapshots.after_call(call, limits()).await.is_some());
    world.snapshots.finish_turn(turn(1), limits()).await;
    let post = tree_of(&world.only_store(), &home, &turn_ref(1, "post")).await;
    assert_eq!(
        post.get("sub").map(String::as_str),
        Some("160000"),
        "a commit of the nested repository"
    );

    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![git_root(&dir)], limits()).await;
    // NOTE: the same size, so git must read the file through its filter.
    write(&sub.join("n.txt"), "m\n");
    write(&dir.join("a.txt"), "again\n");
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    assert!(changes.files.iter().any(|change| change.path == "a.txt"), "{changes:?}");
    let asks = [RootCover::Tracked, RootCover::Path(dir.join("a.txt"))];
    let found =
        world.snapshots.covers(conversation(1), turn(2), &git_root(&dir), &asks, limits()).await;
    assert_eq!(found.len(), 2);
    assert_eq!(found[1], CoverResult::Covered);
    world.snapshots.finish_turn(turn(2), limits()).await;
    assert_eq!(names(&markers), Vec::<String>::new(), "no snapshot ran code");

    // NOTE: the keys act for plain git, so the test proves something.
    for cwd in [&dir, &sub] {
        let _ = efr_stdx::process::command("git", cwd)
            .args(["status", "--porcelain"])
            .env("HOME", &home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .await
            .unwrap();
    }
    let ran = names(&markers);
    assert!(ran.contains(&"fsmonitor".to_owned()), "{ran:?}");
    assert!(ran.contains(&"nested-fsmonitor".to_owned()), "{ran:?}");
    assert!(ran.contains(&"nested-clean".to_owned()), "{ran:?}");
}
