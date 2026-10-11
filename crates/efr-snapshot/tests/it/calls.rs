//! The changes of one call, in a plain directory and in a git project.

use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};

use efr_protocol::{ChangeKind, FileChange, FileChanges};
use efr_snapshot::{LeftOutReason, Limits, Phase, Root};
use pretty_assertions::assert_eq;

use crate::support::{
    World, all_bytes, conversation, git, left_out, limits, meta_of, root, tree_of, turn, turn_ref,
    write,
};

fn file(path: &str, kind: ChangeKind, added: u32, removed: u32) -> FileChange {
    FileChange { path: path.to_owned(), kind, from: None, added, removed, binary: false }
}

/// A shell call that edits, adds, deletes and renames files, and changes a binary file.
fn edit_everything(dir: &std::path::Path) {
    write(&dir.join("keep.txt"), "a\nB\nc\nd\ne\nf\ng\nh\nI\n");
    fs::remove_file(dir.join("gone.txt")).unwrap();
    fs::rename(dir.join("old.txt"), dir.join("new.txt")).unwrap();
    fs::write(dir.join("bin.dat"), [0_u8, 1, 3]).unwrap();
    write(&dir.join("sub/added.txt"), "n\n");
}

fn seed(dir: &std::path::Path) {
    write(&dir.join("keep.txt"), "a\nb\nc\nd\ne\nf\ng\nh\n");
    write(&dir.join("gone.txt"), "one\ntwo\n");
    write(&dir.join("old.txt"), "x\ny\nz\nw\nv\nu\n");
    fs::write(dir.join("bin.dat"), [0_u8, 1, 2]).unwrap();
}

fn expected() -> FileChanges {
    let mut binary = file("bin.dat", ChangeKind::Modified, 0, 0);
    binary.binary = true;
    let mut renamed = file("new.txt", ChangeKind::Renamed, 0, 0);
    renamed.from = Some("old.txt".to_owned());
    FileChanges::from_files(vec![
        binary,
        file("gone.txt", ChangeKind::Deleted, 0, 2),
        file("keep.txt", ChangeKind::Modified, 2, 1),
        renamed,
        file("sub/added.txt", ChangeKind::Added, 1, 0),
    ])
}

#[tokio::test]
async fn a_shell_call_in_a_plain_directory_lists_every_kind_of_change() {
    let world = World::new();
    let dir = world.dir("plain");
    seed(&dir);
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    assert!(!call.is_empty());
    edit_everything(&dir);
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    assert_eq!(changes, expected());
}

#[tokio::test]
async fn a_call_that_changes_nothing_lists_nothing() {
    let world = World::new();
    let dir = world.dir("quiet");
    seed(&dir);
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    assert_eq!(world.snapshots.after_call(call, limits()).await, None);
}

#[tokio::test]
async fn snapshot_leaves_user_index_untouched() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("project");
    seed(&dir);
    write(&dir.join(".gitignore"), "*.log\n");
    git(&dir, &home, &[], &["init", "-q"]).await;
    git(&dir, &home, &[], &["add", "-A"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "seed"]).await;
    write(&dir.join("staged.txt"), "staged\n");
    git(&dir, &home, &[], &["add", "staged.txt"]).await;
    let before = all_bytes(&dir.join(".git"));
    let status = git(&dir, &home, &[], &["status", "--porcelain"]).await;

    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    edit_everything(&dir);
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    assert_eq!(changes, expected());
    let finished = world.snapshots.finish_turn(turn(1), limits()).await.unwrap();
    assert_eq!(finished, expected());

    assert_eq!(all_bytes(&dir.join(".git")), before, "the project's .git and index are unchanged");
    edit_back(&dir);
    assert_eq!(git(&dir, &home, &[], &["status", "--porcelain"]).await, status);
}

/// Undoes [`edit_everything`] by hand, so the project's status can be compared.
fn edit_back(dir: &std::path::Path) {
    write(&dir.join("keep.txt"), "a\nb\nc\nd\ne\nf\ng\nh\n");
    write(&dir.join("gone.txt"), "one\ntwo\n");
    fs::rename(dir.join("new.txt"), dir.join("old.txt")).unwrap();
    fs::write(dir.join("bin.dat"), [0_u8, 1, 2]).unwrap();
    fs::remove_dir_all(dir.join("sub")).unwrap();
}

#[tokio::test]
async fn snapshot_captures_untracked_not_large_ignored() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("ignored");
    write(&dir.join(".gitignore"), ".env\n*.log\ntarget/\nsecret/\n");
    write(&dir.join("main.rs"), "fn main() {}\n");
    write(&dir.join(".env"), "TOKEN=1\n");
    write(&dir.join("small.log"), "log\n");
    fs::write(dir.join("big.log"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
    write(&dir.join("target/debug/app"), "binary\n");
    write(&dir.join("secret/key"), "k\n");
    fs::write(dir.join("huge.bin"), vec![b'y'; 4096]).unwrap();
    symlink("/etc/passwd", dir.join("passwd-link")).unwrap();

    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("main.rs"), "fn main() { run(); }\n");
    world.snapshots.after_call(call, limits()).await.unwrap();
    world.snapshots.finish_turn(turn(1), limits()).await.unwrap();

    let store = world.only_store();
    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(1));
    let tree = tree_of(&store, &home, &pre).await;
    let names: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [".env", ".gitignore", "main.rs", "passwd-link", "secret/key", "small.log"],
        "big.log is above 1 MiB, target/ is a build dir, huge.bin is above the 1 KiB limit"
    );
    assert_eq!(tree["passwd-link"], "120000", "a link is kept as a link");
    assert!(!dir.join(".git").exists(), "a plain directory gets no .git");
    let meta = meta_of(&store, &home, &pre).await;
    assert_eq!(
        left_out(&meta),
        [
            ("big.log".to_owned(), "ignored_too_large".to_owned()),
            ("huge.bin".to_owned(), "too_large".to_owned()),
            ("target".to_owned(), "skipped_dir".to_owned()),
        ],
        "the record names what the snapshot left out, and why"
    );
}

#[tokio::test]
async fn snapshot_captures_small_ignored_env_file() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("env");
    write(&dir.join(".gitignore"), ".env\n.env.*\n");
    write(&dir.join(".env"), "TOKEN=1\n");
    write(&dir.join(".env.local"), "TOKEN=2\n");
    write(&dir.join("app.rs"), "fn main() {}\n");
    let call = world
        .snapshots
        .before_call(conversation(1), turn(1), vec![root(&dir)], Limits::default())
        .await;
    world.snapshots.after_call(call, Limits::default()).await;
    world.snapshots.finish_turn(turn(1), Limits::default()).await;
    let tree = tree_of(&world.only_store(), &home, &turn_ref(1, "pre")).await;
    let names: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [".env", ".env.local", ".gitignore", "app.rs"],
        "the default limits take .env"
    );
}

#[tokio::test]
async fn snapshot_skips_large_files_and_says_so() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("large");
    write(&dir.join("small.txt"), "s\n");
    fs::write(dir.join("dump.sql"), vec![b'd'; 4096]).unwrap();
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    let store = world.only_store();
    let tree = tree_of(&store, &home, &turn_ref(1, "pre")).await;
    assert!(!tree.contains_key("dump.sql"), "above the 1 KiB limit of the test");
    for phase in ["pre", "post"] {
        let meta = meta_of(&store, &home, &turn_ref(1, phase)).await;
        assert_eq!(left_out(&meta), [("dump.sql".to_owned(), "too_large".to_owned())], "{phase}");
    }
    assert_eq!(end.left_out, [(dir.join("dump.sql"), LeftOutReason::TooLarge)]);
    let skipped: Vec<(Phase, u32, u32)> =
        end.snapshots.iter().map(|info| (info.phase, info.files, info.skipped)).collect();
    assert_eq!(skipped, [(Phase::Pre, 1, 1), (Phase::Post, 1, 1)], "one file, one left out");
    assert!(end.kept && end.not_covered.is_empty());
}

#[tokio::test]
async fn snapshot_reads_project_info_exclude() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("excluded");
    git(&dir, &home, &[], &["init", "-q"]).await;
    write(&dir.join(".git/info/exclude"), "notes/\n");
    write(&dir.join("notes/todo.md"), "t\n");
    write(&dir.join("src.rs"), "s\n");
    let off = Limits { ignored_small: false, ..limits() };
    let call = world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], off).await;
    world.snapshots.after_call(call, off).await;
    world.snapshots.finish_turn(turn(1), off).await;
    let store = world.only_store();
    let tree = tree_of(&store, &home, &turn_ref(1, "pre")).await;
    assert_eq!(tree.keys().collect::<Vec<_>>(), ["src.rs"], "notes/ is ignored by info/exclude");
    let meta = meta_of(&store, &home, &turn_ref(1, "pre")).await;
    assert_eq!(left_out(&meta), [("notes".to_owned(), "ignored_off".to_owned())]);
}

#[tokio::test]
async fn snapshot_records_modes_of_private_files() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("modes");
    write(&dir.join(".gitignore"), ".env\n");
    write(&dir.join(".env"), "TOKEN=1\n");
    write(&dir.join("run.sh"), "#!/bin/sh\n");
    write(&dir.join("plain.txt"), "p\n");
    fs::set_permissions(dir.join(".env"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(dir.join("plain.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await;
    let meta = meta_of(&world.only_store(), &home, &turn_ref(1, "pre")).await;
    assert_eq!(meta["modes"], serde_json::json!({ ".env": 0o600, "run.sh": 0o700 }));
}

#[tokio::test]
async fn a_root_with_too_many_files_is_skipped() {
    let world = World::new();
    let dir = world.dir("many");
    for n in 0..5 {
        write(&dir.join(format!("f{n}")), "x\n");
    }
    let few = Limits { max_files: 3, ..limits() };
    let call = world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], few).await;
    assert!(call.is_empty());
    assert_eq!(world.snapshots.finish_turn(turn(1), few).await, None);
}

#[tokio::test]
async fn a_root_with_too_many_files_never_lists_its_ignored_files() {
    let world = World::with_git_log();
    let dir = world.dir("many");
    write(&dir.join(".gitignore"), "*.log\n");
    write(&dir.join("build.log"), "x\n");
    for n in 0..5 {
        write(&dir.join(format!("f{n}")), "x\n");
    }
    let few = Limits { max_files: 3, ..limits() };
    let call = world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], few).await;
    assert!(call.is_empty());
    assert_eq!(
        world.git_runs(),
        ["config", "config", "init", "ls-files"],
        "the format check and one listing; the ignored one never ran"
    );

    // A root below the limit lists its ignored files too, after the check.
    let small = world.dir("small");
    write(&small.join(".gitignore"), "*.log\n");
    write(&small.join("build.log"), "x\n");
    write(&small.join("a.rs"), "x\n");
    let call = world.snapshots.before_call(conversation(1), turn(2), vec![root(&small)], few).await;
    assert!(!call.is_empty());
    let runs = world.git_runs();
    assert_eq!(runs.iter().filter(|run| *run == "ls-files").count(), 2, "{runs:?}");
}

#[tokio::test]
async fn a_skipped_root_is_not_scanned_again_for_a_while() {
    let world = World::new();
    let dir = world.dir("many");
    for n in 0..5 {
        write(
            &dir.join(format!("f{n}")),
            "x
",
        );
    }
    let few = Limits { max_files: 3, ..limits() };
    let call = world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], few).await;
    assert!(call.is_empty());
    for n in 0..3 {
        fs::remove_file(dir.join(format!("f{n}"))).unwrap();
    }
    // NOTE: a scan would find 2 files now; the skip holds without one.
    let call = world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], few).await;
    assert!(call.is_empty(), "the skip holds for a while");
    world.clock.advance(std::time::Duration::from_secs(11 * 60));
    let call = world.snapshots.before_call(conversation(1), turn(3), vec![root(&dir)], few).await;
    assert!(!call.is_empty(), "then the root is scanned again");
}

#[tokio::test]
async fn two_roots_show_their_paths_with_their_prefixes() {
    let world = World::new();
    let project = world.dir("p/app");
    let scratch = world.dir("scratch");
    write(&project.join("a.rs"), "x\n");
    let roots = vec![Root::new(&project, ""), Root::new(&scratch, "$SCRATCH/")];
    let call = world.snapshots.before_call(conversation(1), turn(1), roots, limits()).await;
    write(&project.join("a.rs"), "y\n");
    write(&scratch.join("notes.md"), "n\n");
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    let paths: Vec<&str> = changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["$SCRATCH/notes.md", "a.rs"]);
}

#[tokio::test]
async fn a_leftover_index_lock_does_not_hide_the_next_change() {
    let world = World::new();
    let dir = world.dir("locked");
    write(&dir.join("a.txt"), "1\n");
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    // NOTE: a git that the snapshot timeout or a stop of efrd killed leaves its lock.
    let lock = world.only_store().with_extension("index.lock");
    fs::write(&lock, "").unwrap();

    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "2\n");
    let changes = world.snapshots.after_call(call, limits()).await.unwrap();
    assert_eq!(changes.files, [file("a.txt", ChangeKind::Modified, 1, 1)]);
    assert!(!lock.exists(), "the leftover lock is gone");
    assert!(world.snapshots.finish_turn(turn(1), limits()).await.is_some());
}

#[tokio::test]
async fn a_file_that_grows_past_its_limit_leaves_the_snapshot_and_shows_as_changed() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("grows");
    write(&dir.join(".gitignore"), "*.log\n");
    write(&dir.join("dev.db"), "small\n");
    write(&dir.join("app.log"), "small\n");
    // Ignored files above 1 MiB leave too, below the 4 MiB of untracked files.
    let wide = Limits { max_file_bytes: 4 * 1024 * 1024, ..limits() };
    let call = world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], wide).await;
    fs::write(dir.join("dev.db"), vec![b'd'; 5 * 1024 * 1024]).unwrap();
    fs::write(dir.join("app.log"), vec![b'l'; 2 * 1024 * 1024]).unwrap();
    let changes = world.snapshots.after_call(call, wide).await.unwrap();
    let both =
        [file("app.log", ChangeKind::Modified, 0, 0), file("dev.db", ChangeKind::Modified, 0, 0)];
    assert_eq!(changes.files, both, "changed, with no line counts");

    // The next call does not hash the large file again, and lists nothing for it.
    let call = world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], wide).await;
    fs::write(dir.join("dev.db"), vec![b'e'; 6 * 1024 * 1024]).unwrap();
    assert_eq!(world.snapshots.after_call(call, wide).await, None);

    let turn_changes = world.snapshots.finish_turn(turn(1), wide).await.unwrap();
    assert_eq!(turn_changes.files, both);
    let post = format!("refs/efr/{}/{}/post", conversation(1), turn(1));
    let tree = tree_of(&world.only_store(), &home, &post).await;
    assert_eq!(tree.keys().collect::<Vec<_>>(), [".gitignore"], "both large files are left out");
}

/// The lists say what changed in a root while a call or a turn ran: a snapshot cannot
/// tell who made a change, so another conversation's write at the same time shows in
/// both. The docs say so (the README of this crate and of efr-cli).
#[tokio::test]
async fn two_conversations_on_one_project_each_see_what_changed_during_their_call() {
    let world = World::new();
    let dir = world.dir("shared");
    write(&dir.join("a.txt"), "a\n");
    write(&dir.join("b.txt"), "b\n");
    let first =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    let second =
        world.snapshots.before_call(conversation(2), turn(2), vec![root(&dir)], limits()).await;
    write(&dir.join("b.txt"), "b2\n");
    let changes = world.snapshots.after_call(second, limits()).await.unwrap();
    assert_eq!(changes.files, [file("b.txt", ChangeKind::Modified, 1, 1)]);
    write(&dir.join("a.txt"), "a2\n");
    let changes = world.snapshots.after_call(first, limits()).await.unwrap();
    assert_eq!(
        changes.files,
        [file("a.txt", ChangeKind::Modified, 1, 1), file("b.txt", ChangeKind::Modified, 1, 1)],
        "the first call's list holds the second conversation's write too"
    );
    let turn_one = world.snapshots.finish_turn(turn(1), limits()).await.unwrap();
    assert_eq!(turn_one.files.len(), 2, "and so does its turn");
    let turn_two = world.snapshots.finish_turn(turn(2), limits()).await.unwrap();
    assert_eq!(turn_two.files.len(), 2);
}
