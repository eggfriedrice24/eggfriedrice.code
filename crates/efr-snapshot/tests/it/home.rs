//! The home store: copies of paths outside the roots before and after a call, and the
//! home part of an undo and a redo.

use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::UndoReason;
use efr_snapshot::{
    Expect, HomeEntries, HomeEntry, HomeReader, HomeState, HomeStep, SnapshotError, UndoPlan,
    blob_id,
};
use pretty_assertions::assert_eq;

use crate::support::{World, conversation, git, limits, root, turn, write};
use crate::undo::{apply_root, expect_now, undo_id};

/// The entry of `path` as the daemon reads it: a link as a link, a file with its bytes.
fn read_entry(path: &Path, floor: bool) -> (HomeEntry, Option<Vec<u8>>) {
    let entry = |state| HomeEntry { path: path.to_path_buf(), state, floor, chain: Vec::new() };
    match fs::symlink_metadata(path) {
        Err(_) => (entry(HomeState::Missing), None),
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = fs::read_link(path).unwrap().as_os_str().as_bytes().to_vec();
            (entry(HomeState::Link { target }), None)
        }
        Ok(meta) => (
            entry(HomeState::File { mode: meta.permissions().mode() & 0o7777 }),
            Some(fs::read(path).unwrap()),
        ),
    }
}

fn entries(paths: &[&Path]) -> HomeEntries {
    HomeEntries::new(paths.iter().map(|path| read_entry(path, false)).collect())
}

/// The home store's git directory.
fn home_git(world: &World) -> PathBuf {
    world.store_dir().join("home.git")
}

/// Applies the home steps of `plan` that act, as the daemon does.
async fn apply_home(world: &World, plan: &UndoPlan) {
    for step in plan.home.steps.iter().filter(|step| step.acts()) {
        if step.expect != Expect::Any && expect_now(&step.path) != step.expect {
            continue;
        }
        let _ = fs::remove_file(&step.path);
        match &step.write {
            HomeState::File { mode } => {
                let bytes = world.snapshots.home_blob(step.blob.as_ref().unwrap()).await.unwrap();
                fs::write(&step.path, bytes).unwrap();
                fs::set_permissions(&step.path, fs::Permissions::from_mode(*mode & 0o777)).unwrap();
            }
            HomeState::Link { target } => {
                std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(target), &step.path)
                    .unwrap();
            }
            HomeState::Missing => {}
            other => unreachable!("{other:?}"),
        }
    }
}

fn step<'a>(plan: &'a UndoPlan, path: &Path) -> &'a HomeStep {
    plan.home
        .steps
        .iter()
        .find(|step| step.path == path)
        .unwrap_or_else(|| panic!("no step for {}: {plan:#?}", path.display()))
}

async fn plan(world: &World, force: bool) -> UndoPlan {
    world
        .snapshots
        .plan_undo(conversation(1), turn(1), &[], force, &undo_id(1), limits())
        .await
        .unwrap()
}

#[tokio::test]
async fn home_store_keeps_link_and_target_apart() {
    let world = World::new();
    let home = world.home();
    write(&home.join("dotfiles/zshrc"), "alias k=kubectl\n");
    std::os::unix::fs::symlink("dotfiles/zshrc", home.join(".zshrc")).unwrap();
    let link = home.join(".zshrc");
    let real = home.join("dotfiles/zshrc");
    let (mut first, _) = read_entry(&link, true);
    first.chain = vec![real.clone()];
    let copied = HomeEntries::new(vec![(first, None), read_entry(&real, true)]);
    let copy = world.snapshots.home_before(conversation(1), turn(1), copied).await.unwrap();
    assert_eq!(copy.n, 1);

    let env = [("GIT_DIR", home_git(&world))];
    let env: Vec<(&str, &Path)> = env.iter().map(|(name, path)| (*name, path.as_path())).collect();
    let tree = git(&home, &home, &env, &["ls-tree", "-r", copy.before.as_str()]).await;
    let tree_of = |path: &Path| format!("files{}", path.display());
    assert!(
        tree.contains(&format!("120000 blob {}\t{}", blob_id(b"dotfiles/zshrc"), tree_of(&link))),
        "{tree}"
    );
    assert!(
        tree.contains(&format!(
            "100644 blob {}\t{}",
            blob_id(b"alias k=kubectl\n"),
            tree_of(&real)
        )),
        "{tree}"
    );
    let spec = format!("{}:efr-home.json", copy.before);
    let text = git(&home, &home, &env, &["cat-file", "blob", &spec]).await;
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest["phase"], "before");
    assert_eq!(manifest["entries"][0]["kind"], "link");
    assert_eq!(manifest["entries"][0]["floor"], true);
    assert_eq!(manifest["entries"][0]["chain"][0], real.to_str().unwrap());
    assert_eq!(manifest["entries"][1]["kind"], "file");
    assert_eq!(manifest["entries"][1]["mode"], 0o644);
    assert!(!home_git(&world).with_extension("index").exists(), "the home store has no index");
}

#[tokio::test]
async fn home_store_records_missing_path() {
    let world = World::new();
    let path = world.home().join("outside/b.txt");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let copy = world.snapshots.home_before(conversation(1), turn(1), entries(&[&path])).await;
    let copy = copy.unwrap();
    write(&path, "hello\n");
    let deltas = world.snapshots.home_after(&copy, entries(&[&path])).await.unwrap();
    assert_eq!((deltas[0].changed, deltas[0].covered), (true, true));

    let plan = plan(&world, false).await;
    let made = step(&plan, &path);
    assert_eq!(made.write, HomeState::Missing);
    assert_eq!(made.expect, Expect::Blob(blob_id(b"hello\n")));
    assert_eq!(made.trash, Some((blob_id(b"hello\n"), 0o100_644)), "the removal keeps its bytes");
    apply_home(&world, &plan).await;
    assert!(!path.exists());
}

#[tokio::test]
async fn home_after_lists_new_paths_as_missing() {
    let world = World::new();
    let dir = world.home().join("Documents");
    write(&dir.join("a.txt"), "a\n");
    let copy = world
        .snapshots
        .home_before(conversation(1), turn(1), entries(&[&dir.join("a.txt")]))
        .await
        .unwrap();
    write(&dir.join("b.txt"), "made by the call\n");
    let after = entries(&[&dir.join("a.txt"), &dir.join("b.txt")]);
    let deltas = world.snapshots.home_after(&copy, after).await.unwrap();
    let changed: Vec<(PathBuf, bool)> =
        deltas.iter().map(|delta| (delta.path.clone(), delta.changed)).collect();
    assert_eq!(changed, [(dir.join("a.txt"), false), (dir.join("b.txt"), true)]);

    let plan = plan(&world, false).await;
    assert_eq!(plan.home.steps.len(), 1, "a.txt did not change: {plan:#?}");
    assert_eq!(step(&plan, &dir.join("b.txt")).write, HomeState::Missing);
}

#[tokio::test]
async fn undo_of_two_home_copies_restores_the_first_state() {
    let world = World::new();
    let path = world.home().join(".zshrc");
    write(&path, "v1\n");
    for next in ["v2\n", "v3\n"] {
        let copy = world.snapshots.home_before(conversation(1), turn(1), entries(&[&path])).await;
        let copy = copy.unwrap();
        write(&path, next);
        world.snapshots.home_after(&copy, entries(&[&path])).await.unwrap();
    }
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert!(turns[0].home && turns[0].roots.is_empty(), "{turns:?}");

    let undo = plan(&world, false).await;
    let restore = step(&undo, &path);
    assert_eq!(restore.write, HomeState::File { mode: 0o644 });
    assert_eq!(restore.blob, Some(blob_id(b"v1\n")));
    assert_eq!(restore.expect, Expect::Blob(blob_id(b"v3\n")));
    world.snapshots.capture_before_undo(&undo, limits()).await.unwrap();
    world
        .snapshots
        .home_undo_before(conversation(1), &undo.undo_id, entries(&[&path]))
        .await
        .unwrap();
    apply_home(&world, &undo).await;
    world.snapshots.finish_undo(&undo, limits()).await.unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "v1\n");
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    assert!(turns[0].undone, "{turns:?}");

    let redo = world.snapshots.plan_redo(conversation(1), &[], false, limits()).await.unwrap();
    let back = step(&redo, &path);
    assert_eq!(
        (back.blob.clone(), back.expect.clone()),
        (Some(blob_id(b"v3\n")), Expect::Blob(blob_id(b"v1\n")))
    );
    world.snapshots.capture_before_undo(&redo, limits()).await.unwrap();
    apply_home(&world, &redo).await;
    world.snapshots.finish_undo(&redo, limits()).await.unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "v3\n");
    assert!(!world.snapshots.turns_with_refs(conversation(1)).await[0].undone);
}

#[tokio::test]
async fn home_path_in_a_root_belongs_to_the_root_plan() {
    let world = World::new();
    let dir = world.dir("p");
    let inside = dir.join("x.txt");
    let outside = world.home().join("y.txt");
    write(&inside, "x\n");
    write(&outside, "y\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    let copy = world
        .snapshots
        .home_before(conversation(1), turn(1), entries(&[&inside, &outside]))
        .await
        .unwrap();
    write(&inside, "x2\n");
    write(&outside, "y2\n");
    world.snapshots.home_after(&copy, entries(&[&inside, &outside])).await.unwrap();
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.turn_end(turn(1), limits()).await;

    let plan = plan(&world, false).await;
    let home: Vec<&Path> = plan.home.steps.iter().map(|step| step.path.as_path()).collect();
    assert_eq!(home, [outside.as_path()]);
    let roots: Vec<&str> = plan.roots[0].entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(roots, ["x.txt"]);
    world.snapshots.capture_before_undo(&plan, limits()).await.unwrap();
    apply_root(&world.snapshots, &plan.roots[0]).await;
    apply_home(&world, &plan).await;
    world.snapshots.finish_undo(&plan, limits()).await.unwrap();
    assert_eq!(fs::read_to_string(&inside).unwrap(), "x\n");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "y\n");
}

#[tokio::test]
async fn home_copy_without_after_is_not_covered() {
    let world = World::new();
    let path = world.home().join("notes.txt");
    write(&path, "before\n");
    world.snapshots.home_before(conversation(1), turn(1), entries(&[&path])).await.unwrap();
    write(&path, "the call stopped here\n");

    let plan = plan(&world, false).await;
    let skipped = step(&plan, &path);
    assert_eq!(skipped.write, HomeState::NotCovered(UndoReason::CallUnfinished));
    assert!(!skipped.acts());
    let forced = self::plan(&world, true).await;
    let forced = step(&forced, &path);
    assert_eq!(
        (forced.expect.clone(), forced.write.clone()),
        (Expect::Any, HomeState::File { mode: 0o644 })
    );
    assert!(forced.acts());
}

/// A reader that fails at its second entry.
struct Failing(u32);

impl HomeReader for Failing {
    fn next(
        &mut self,
    ) -> Option<Result<(HomeEntry, Option<Box<dyn std::io::Read + Send>>), SnapshotError>> {
        self.0 += 1;
        if self.0 > 1 {
            let path = PathBuf::from("/nowhere");
            let source = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
            return Some(Err(SnapshotError::Io { path, source }));
        }
        let entry = HomeEntry {
            path: PathBuf::from("/tmp/x"),
            state: HomeState::File { mode: 0o644 },
            floor: false,
            chain: Vec::new(),
        };
        Some(Ok((entry, Some(Box::new(std::io::Cursor::new(b"x".to_vec()))))))
    }
}

#[tokio::test]
async fn a_failed_read_moves_no_ref_of_a_copy() {
    let world = World::new();
    let failed = world.snapshots.home_before(conversation(1), turn(1), Failing(0)).await;
    assert!(matches!(failed, Err(SnapshotError::Io { .. })), "{failed:?}");
    assert!(world.snapshots.turns_with_refs(conversation(1)).await.is_empty());
    let bad = HomeEntries::new(vec![(
        HomeEntry {
            path: PathBuf::from("relative/path"),
            state: HomeState::Missing,
            floor: false,
            chain: Vec::new(),
        },
        None,
    )]);
    let refused = world.snapshots.home_before(conversation(1), turn(1), bad).await;
    assert!(matches!(refused, Err(SnapshotError::BadHomeEntry { .. })), "{refused:?}");
}

#[tokio::test]
async fn home_copy_runs_no_hook_of_the_store() {
    let world = World::new();
    let path = world.home().join("a.txt");
    write(&path, "a\n");
    world.snapshots.home_before(conversation(1), turn(1), entries(&[&path])).await.unwrap();
    let marker = world.base.path().join("hook-ran");
    let hooks = home_git(&world).join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    for hook in ["reference-transaction", "post-commit", "post-receive"] {
        let script = hooks.join(hook);
        fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let copy = world.snapshots.home_before(conversation(1), turn(1), entries(&[&path])).await;
    assert_eq!(copy.unwrap().n, 2);
    assert!(!marker.exists(), "fast-import ran a hook");
}

#[tokio::test]
async fn an_empty_copy_has_no_step() {
    let world = World::new();
    let none = HomeEntries::default();
    let copy = world.snapshots.home_before(conversation(1), turn(1), none).await.unwrap();
    let deltas = world.snapshots.home_after(&copy, HomeEntries::default()).await.unwrap();
    assert!(deltas.is_empty());
    assert!(plan(&world, false).await.home.steps.is_empty());
}
