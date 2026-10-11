//! The mirror of a project's refs at the turn's first snapshot of a git root.

use std::fs;
use std::path::Path;

use efr_snapshot::{GitDirs, Limits, NOT_COVERED_REFS, Root};
use pretty_assertions::assert_eq;

use crate::support::{
    World, conversation, git, git_root, limits, meta_of, project, refs, turn, turn_ref, write,
};

/// `refs/efr-mirror/<conversation 1>/<turn n>/<name>`.
fn mirror_ref(n: u128, name: &str) -> String {
    format!("refs/efr-mirror/{}/{}/{name}", conversation(1), turn(n))
}

/// The mirror refs of turn `n` in the store at `store`.
async fn mirrored(store: &Path, home: &Path, n: u128) -> Vec<String> {
    let prefix = mirror_ref(n, "");
    refs(store, home).await.into_iter().filter(|name| name.starts_with(&prefix)).collect()
}

fn not_covered(dir: &Path, reason: &str) -> Vec<(std::path::PathBuf, String)> {
    vec![(dir.to_path_buf(), format!("{NOT_COVERED_REFS}{reason}"))]
}

/// Bytes that do not compress.
fn noise(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

#[tokio::test]
async fn snapshot_mirror_takes_heads_tags_and_stash() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "app").await;
    git(&dir, &home, &[], &["tag", "v1"]).await;
    git(&dir, &home, &[], &["branch", "feature"]).await;
    git(&dir, &home, &[], &["update-ref", "refs/remotes/origin/main", "HEAD"]).await;
    write(&dir.join("a.txt"), "stashed\n");
    git(&dir, &home, &[], &["stash", "-q"]).await;
    let main = git(&dir, &home, &[], &["rev-parse", "main"]).await.trim().to_owned();

    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![git_root(&dir)], limits()).await;
    let store = world.only_store();
    assert_eq!(
        mirrored(&store, &home, 1).await,
        [
            mirror_ref(1, "heads/feature"),
            mirror_ref(1, "heads/main"),
            mirror_ref(1, "stash"),
            mirror_ref(1, "tags/v1"),
        ],
        "heads, tags and the stash; no remote ref"
    );
    let pre = meta_of(&store, &home, &turn_ref(1, "pre")).await;
    assert_eq!(pre["mirror"], "taken");
    assert_eq!(pre["head"], "refs/heads/main");
    assert_eq!(pre["refs"]["HEAD"], main.as_str());
    assert_eq!(pre["refs"]["refs/heads/main"], main.as_str());
    assert_eq!(pre["git"]["git_dir"], dir.join(".git").to_str().unwrap());

    // A commit in the turn: post records the new value.
    write(&dir.join("b.txt"), "b\n");
    git(&dir, &home, &[], &["add", "b.txt"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "two"]).await;
    world.snapshots.after_call(call, limits()).await;
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(end.not_covered, Vec::new());
    let post = meta_of(&store, &home, &turn_ref(1, "post")).await;
    assert_ne!(post["refs"]["refs/heads/main"], main.as_str());
    assert_eq!(post["refs"]["refs/heads/feature"], main.as_str());
}

#[tokio::test]
async fn snapshot_mirror_skipped_above_mirror_max_mib() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "big").await;
    let small = Limits { mirror_max_bytes: 1, ..limits() };
    world
        .snapshots
        .before_call(conversation(1), turn(1), vec![git_root(&dir)], small)
        .await
        .discard();
    let end = world.snapshots.turn_end(turn(1), small).await;
    let store = world.only_store();
    assert_eq!(mirrored(&store, &home, 1).await, Vec::<String>::new());
    assert_eq!(end.not_covered, not_covered(&dir, "over snapshot.mirror_max_mib"));
    assert!(end.kept, "the files are still covered");

    let off = Limits { mirror_max_bytes: 0, ..limits() };
    world
        .snapshots
        .before_call(conversation(1), turn(2), vec![git_root(&dir)], off)
        .await
        .discard();
    let end = world.snapshots.turn_end(turn(2), off).await;
    assert_eq!(end.not_covered, not_covered(&dir, "snapshot.mirror_max_mib is 0"));
}

#[tokio::test]
async fn mirror_over_turn_budget_is_dropped() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "linked-objects").await;
    fs::write(dir.join("noise.bin"), noise(64 * 1024)).unwrap();
    git(&dir, &home, &[], &["add", "noise.bin"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "noise"]).await;
    // NOTE: the walk of the object dir follows no link, so it counts nothing here; the
    // budget of the fetch still holds.
    let elsewhere = world.dir("objects-elsewhere");
    fs::remove_dir(&elsewhere).unwrap();
    fs::rename(dir.join(".git/objects"), &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join(".git/objects")).unwrap();

    let budget = Limits { mirror_max_bytes: 4096, max_file_bytes: 1024 * 1024, ..limits() };
    world
        .snapshots
        .before_call(conversation(1), turn(1), vec![git_root(&dir)], budget)
        .await
        .discard();
    let end = world.snapshots.turn_end(turn(1), budget).await;
    assert_eq!(mirrored(&world.only_store(), &home, 1).await, Vec::<String>::new());
    assert_eq!(end.not_covered, not_covered(&dir, "mirror over budget"));
}

#[tokio::test]
async fn snapshot_mirror_of_a_linked_work_tree() {
    let world = World::new();
    let home = world.home();
    let main = project(&world, "main").await;
    let linked = world.home().join("linked");
    let path = linked.to_str().unwrap();
    git(&main, &home, &[], &["worktree", "add", "-q", "-b", "feature", path]).await;
    let linked = fs::canonicalize(linked).unwrap();
    let dirs =
        GitDirs { git_dir: main.join(".git/worktrees/linked"), common_dir: main.join(".git") };
    let root = Root::new(&linked, "").with_git(Some(dirs));

    world.snapshots.before_call(conversation(1), turn(1), vec![root], limits()).await.discard();
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(end.not_covered, Vec::new());
    let store = world.only_store();
    assert_eq!(
        mirrored(&store, &home, 1).await,
        [mirror_ref(1, "heads/feature"), mirror_ref(1, "heads/main")]
    );
    let pre = meta_of(&store, &home, &turn_ref(1, "pre")).await;
    assert_eq!(pre["head"], "refs/heads/feature", "the linked work tree's own HEAD");
}

#[tokio::test]
async fn mirror_refuses_a_planted_git_file() {
    let world = World::new();
    let home = world.home();
    let other = project(&world, "other").await;
    let dir = world.dir("planted");
    write(&dir.join("a.txt"), "a\n");
    write(&dir.join(".git"), &format!("gitdir: {}\n", other.join(".git").display()));

    // NOTE: the daemon trusts no git dir for this root, so it names none.
    world
        .snapshots
        .before_call(conversation(1), turn(1), vec![Root::new(&dir, "")], limits())
        .await
        .discard();
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(end.not_covered, not_covered(&dir, "the git dir is not trusted"));
    let store = world.only_store();
    assert_eq!(mirrored(&store, &home, 1).await, Vec::<String>::new());
    let pre = meta_of(&store, &home, &turn_ref(1, "pre")).await;
    assert_eq!(pre["git"], serde_json::Value::Null);
    assert_eq!(pre["refs"], serde_json::json!({}));
}

#[tokio::test]
async fn mirror_refuses_alternates() {
    let world = World::new();
    let home = world.home();
    let origin = project(&world, "origin").await;
    let shared = world.home().join("shared");
    let (from, to) = (origin.to_str().unwrap(), shared.to_str().unwrap());
    git(&home, &home, &[], &["clone", "-q", "--shared", from, to]).await;
    let shared = fs::canonicalize(shared).unwrap();
    assert!(shared.join(".git/objects/info/alternates").exists());

    world
        .snapshots
        .before_call(conversation(1), turn(1), vec![git_root(&shared)], limits())
        .await
        .discard();
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(end.not_covered, not_covered(&shared, "alternates"));
    assert_eq!(mirrored(&world.only_store(), &home, 1).await, Vec::<String>::new());
}

#[tokio::test]
async fn snapshot_mirror_skipped_for_a_shallow_clone() {
    let world = World::new();
    let home = world.home();
    let origin = project(&world, "deep").await;
    write(&origin.join("a.txt"), "two\n");
    git(&origin, &home, &[], &["commit", "-q", "-am", "two"]).await;
    let shallow = world.home().join("shallow");
    let from = format!("file://{}", origin.display());
    git(&home, &home, &[], &["clone", "-q", "--depth", "1", &from, shallow.to_str().unwrap()])
        .await;
    let shallow = fs::canonicalize(shallow).unwrap();

    world
        .snapshots
        .before_call(conversation(1), turn(1), vec![git_root(&shallow)], limits())
        .await
        .discard();
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert_eq!(end.not_covered, not_covered(&shallow, "shallow"));
}
