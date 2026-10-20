//! The collector: the newest turns of each conversation stay, idle stores go.

use std::time::Duration;

use pretty_assertions::assert_eq;

use crate::support::{
    World, conversation, git, git_root, limits, project, refs, root, stores, turn, write,
};

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
    world
        .snapshots
        .before_call(conversation(1), turn(1), vec![root(&dir)], limits())
        .await
        .discard();
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

/// A bare repository `home.git` in the store directory, as the home store is, with the
/// refs of a turn of conversation 1 that is newer than every turn of the test.
async fn home_store(world: &World) -> std::path::PathBuf {
    let home = world.home();
    let git_dir = world.store_dir().join("home.git");
    std::fs::create_dir_all(world.store_dir()).unwrap();
    git(&home, &home, &[], &["init", "-q", "--bare", git_dir.to_str().unwrap()]).await;
    let env = [("GIT_DIR", git_dir.as_path())];
    let tree = git(&home, &home, &env, &["mktree"]).await;
    let commit = git(&home, &home, &env, &["commit-tree", tree.trim(), "-m", "home"]).await;
    for name in ["pre", "post"] {
        let name = format!("refs/efr/{}/{}/home/1/{name}", conversation(1), turn(99));
        git(&home, &home, &env, &["update-ref", &name, commit.trim()]).await;
    }
    git_dir
}

#[tokio::test]
async fn stores_skip_the_home_store() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    let home_git = home_store(&world).await;
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    write(&dir.join("a.txt"), "1\n");
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await.unwrap();

    let newest = world.snapshots.turn_diff(conversation(1), None, false, 100).await.unwrap();
    assert_eq!(newest.unwrap().turn_id, turn(1), "the home store is no root store");
    // NOTE: the home store's turn is the newest of the conversation, so it stays.
    let report = world.snapshots.gc(1, KEEP_LONG).await;
    assert_eq!(report.refs_deleted, 2, "only the root store's pre and post");
    assert_eq!(refs(&home_git, &home).await.len(), 2, "the home store keeps its refs");
}

#[tokio::test]
async fn gc_never_deletes_home_store_without_root_file() {
    let world = World::new();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    let home_git = home_store(&world).await;
    world
        .snapshots
        .before_call(conversation(1), turn(1), vec![root(&dir)], limits())
        .await
        .discard();
    world.snapshots.finish_turn(turn(1), limits()).await;
    world.clock.advance(KEEP_LONG + Duration::from_secs(1));
    let report = world.snapshots.gc(50, KEEP_LONG).await;
    assert_eq!(report.stores_deleted, 1, "the idle root store goes");
    assert!(home_git.join("HEAD").is_file(), "the home store, which has no root file, stays");
}

#[tokio::test]
async fn gc_packs_loose_objects() {
    let world = World::new();
    let dir = world.dir("many");
    for n in 0..8000 {
        write(&dir.join(format!("d{}/f{n}", n % 40)), &format!("{n}\n"));
    }
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await;
    let store = world.only_store();
    let loose = || {
        std::fs::read_dir(store.join("objects"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().len() == 2)
            .map(|entry| std::fs::read_dir(entry.path()).unwrap().count())
            .sum::<usize>()
    };
    assert!(loose() > 6700, "{} loose objects", loose());

    world.snapshots.gc(50, KEEP_LONG).await;
    assert!(loose() < 100, "{} loose objects after the collection", loose());
    let packs = std::fs::read_dir(store.join("objects/pack"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "pack"))
        .count();
    assert_eq!(packs, 1);
    // The snapshots still work.
    let call =
        world.snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    write(&dir.join("d0/f0"), "changed\n");
    assert!(world.snapshots.after_call(call, limits()).await.is_some());
}

#[tokio::test]
async fn gc_drops_the_mirror_with_its_turn() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "p").await;
    for n in 1..=2 {
        let roots = vec![git_root(&dir)];
        world.snapshots.before_call(conversation(1), turn(n), roots, limits()).await.discard();
        world.snapshots.finish_turn(turn(n), limits()).await;
    }
    let report = world.snapshots.gc(1, KEEP_LONG).await;
    let left = refs(&world.only_store(), &home).await;
    assert_eq!(report.refs_deleted, 3, "pre, post and the mirror of turn 1: {left:?}");
    assert!(left.iter().all(|name| name.contains(&turn(2).to_string())), "{left:?}");
}

/// Every ref of the store at `git_dir` that names turn `n` of conversation 1.
async fn refs_of_turn(git_dir: &std::path::Path, home: &std::path::Path, n: u128) -> Vec<String> {
    let name = turn(n).to_string();
    refs(git_dir, home).await.into_iter().filter(|refname| refname.contains(&name)).collect()
}

#[tokio::test]
async fn gc_keeps_a_turn_whole_across_stores() {
    let world = World::new();
    let home = world.home();
    let first = world.dir("a");
    let second = world.dir("b");
    for dir in [&first, &second] {
        write(&dir.join("x.txt"), "0\n");
    }
    for (n, dirs) in [(1, vec![&first, &second]), (2, vec![&first]), (3, vec![&second])] {
        let roots = dirs.iter().map(|dir| root(dir)).collect();
        let call = world.snapshots.before_call(conversation(1), turn(n), roots, limits()).await;
        for dir in dirs {
            write(&dir.join("x.txt"), &format!("{n}\n"));
        }
        world.snapshots.after_call(call, limits()).await;
        world.snapshots.finish_turn(turn(n), limits()).await;
    }
    let all = stores(&world.store_dir());
    assert_eq!(all.len(), 2);

    // Each store alone holds two turns, so a count per store would keep turn 1.
    let report = world.snapshots.gc(2, KEEP_LONG).await;
    assert_eq!(report.refs_deleted, 4, "pre and post of turn 1 in both stores");
    for store in &all {
        assert_eq!(refs_of_turn(store, &home, 1).await, Vec::<String>::new());
    }
    let turns = world.snapshots.turns_with_refs(conversation(1)).await;
    let left: Vec<_> = turns.iter().map(|refs| refs.turn).collect();
    assert_eq!(left, [turn(3), turn(2)]);
}

#[tokio::test]
async fn gc_keeps_the_home_store() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    let file = home.join("outside.txt");
    write(&file, "before\n");
    let entry = |text: &str| {
        efr_snapshot::HomeEntries::new(vec![(
            efr_snapshot::HomeEntry {
                path: file.clone(),
                state: efr_snapshot::HomeState::File { mode: 0o644 },
                floor: false,
                chain: Vec::new(),
                missing_parents: Vec::new(),
            },
            Some(text.as_bytes().to_vec()),
        )])
    };
    for n in 1..=2 {
        let copy = world.snapshots.home_before(conversation(1), turn(n), entry("x")).await;
        world.snapshots.home_after(&copy.unwrap(), entry("y")).await.unwrap();
    }
    let call =
        world.snapshots.before_call(conversation(1), turn(3), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(3), limits()).await;
    let home_git = world.store_dir().join("home.git");

    let report = world.snapshots.gc(2, KEEP_LONG).await;
    assert_eq!((report.refs_deleted, report.stores_deleted), (1, 0), "turn 1's copy");
    assert_eq!(
        refs(&home_git, &home).await,
        [format!("refs/efr/{}/{}/home/1", conversation(1), turn(2))]
    );

    // The home store counts its own idle days from its last copy.
    world.clock.advance(KEEP_LONG / 2);
    let copy = world.snapshots.home_before(conversation(1), turn(4), entry("z")).await.unwrap();
    world.snapshots.home_after(&copy, entry("z")).await.unwrap();
    world.clock.advance(KEEP_LONG / 2 + Duration::from_secs(1));
    let report = world.snapshots.gc(50, KEEP_LONG).await;
    assert_eq!(report.stores_deleted, 1, "only the idle root store");
    assert!(home_git.join("HEAD").is_file());
    world.clock.advance(KEEP_LONG);
    assert_eq!(world.snapshots.gc(50, KEEP_LONG).await.stores_deleted, 1);
    assert!(!home_git.exists(), "an idle home store goes too");
}

#[tokio::test]
async fn gc_trims_undo_refs_with_their_turn() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("p");
    write(&dir.join("a.txt"), "0\n");
    for n in 1..=3 {
        let call =
            world.snapshots.before_call(conversation(1), turn(n), vec![root(&dir)], limits()).await;
        write(&dir.join("a.txt"), &format!("{n}\n"));
        world.snapshots.after_call(call, limits()).await;
        world.snapshots.finish_turn(turn(n), limits()).await;
        if n == 1 {
            let plan = crate::undo::plan(&world, 1, false, 1).await;
            crate::undo::apply(&world, &plan).await;
        }
    }
    let store = world.only_store();
    let undo_refs = |all: Vec<String>| -> Vec<String> {
        all.into_iter().filter(|name| name.starts_with("refs/efr-undo/")).collect()
    };
    assert_eq!(undo_refs(refs(&store, &home).await).len(), 2, "before and after");

    let report = world.snapshots.gc(2, KEEP_LONG).await;
    assert_eq!(report.refs_deleted, 5, "pre, post, undone, before and after");
    assert_eq!(undo_refs(refs(&store, &home).await), Vec::<String>::new());
    assert_eq!(refs_of_turn(&store, &home, 1).await, Vec::<String>::new());
}

#[tokio::test]
async fn gc_caps_mirror_refs_of_a_store() {
    let world = World::new();
    let home = world.home();
    let dir = project(&world, "app").await;
    git(&dir, &home, &[], &["checkout", "-q", "-b", "old"]).await;
    std::fs::write(dir.join("noise.bin"), noise(200 * 1024)).unwrap();
    git(&dir, &home, &[], &["add", "noise.bin"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "noise"]).await;
    git(&dir, &home, &[], &["checkout", "-q", "main"]).await;
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![git_root(&dir)], limits()).await;
    git(&dir, &home, &[], &["branch", "-q", "-D", "old"]).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await;
    world
        .snapshots
        .before_call(conversation(1), turn(2), vec![git_root(&dir)], limits())
        .await
        .discard();
    world.snapshots.finish_turn(turn(2), limits()).await;
    let store = world.only_store();
    let mirrors = |all: Vec<String>, n: u128| -> usize {
        let prefix = format!("refs/efr-mirror/{}/{}/", conversation(1), turn(n));
        all.into_iter().filter(|name| name.starts_with(&prefix)).count()
    };
    assert_eq!(mirrors(refs(&store, &home).await, 1), 2, "main and old");

    // Four times 25 KB: the old turn's mirror alone reaches 200 KiB that no snapshot
    // holds.
    let report = world.snapshots.gc_capped(50, KEEP_LONG, 25_000).await;
    assert_eq!(report.refs_deleted, 2);
    let left = refs(&store, &home).await;
    assert_eq!((mirrors(left.clone(), 1), mirrors(left, 2)), (0, 1), "the newest mirror stays");
    let plan = world
        .snapshots
        .plan_undo(conversation(1), turn(1), &[git_root(&dir)], false, "u-1", limits())
        .await
        .unwrap();
    let old = plan.roots[0].refs.iter().find(|ref_plan| ref_plan.name == "refs/heads/old");
    assert_eq!(old.unwrap().reason, Some(efr_protocol::UndoReason::StoreFull));
}

/// Bytes that do not compress.
fn noise(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
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
async fn gc_keeps_the_home_copy_of_an_undo_with_its_turn() {
    let world = World::new();
    let home = world.home();
    let file = home.join(".zshrc");
    write(&file, "v1\n");
    let read = |text: &str| {
        efr_snapshot::HomeEntries::new(vec![(
            efr_snapshot::HomeEntry {
                path: file.clone(),
                state: efr_snapshot::HomeState::File { mode: 0o644 },
                floor: true,
                chain: Vec::new(),
                missing_parents: Vec::new(),
            },
            Some(text.as_bytes().to_vec()),
        )])
    };
    let copy = world.snapshots.home_before(conversation(1), turn(1), read("v1\n")).await.unwrap();
    world.snapshots.home_after(&copy, read("v2\n")).await.unwrap();
    write(&file, "v2\n");
    let plan = crate::undo::plan(&world, 1, false, 1).await;
    world.snapshots.capture_before_undo(&plan, limits()).await.unwrap();
    world.snapshots.home_undo_before(conversation(1), &plan.undo_id, read("v2\n")).await.unwrap();
    world.snapshots.finish_undo(&plan, limits()).await.unwrap();
    let home_git = world.store_dir().join("home.git");
    let copy_ref = format!("refs/efr-undo/{}/{}/home", conversation(1), plan.undo_id);
    assert!(refs(&home_git, &home).await.contains(&copy_ref));

    let dir = world.dir("p");
    write(&dir.join("a.txt"), "a\n");
    world
        .snapshots
        .before_call(conversation(1), turn(2), vec![root(&dir)], limits())
        .await
        .discard();
    world.snapshots.finish_turn(turn(2), limits()).await;
    world.snapshots.gc(2, KEEP_LONG).await;
    assert!(refs(&home_git, &home).await.contains(&copy_ref), "the turn stays, so its undo does");
    world.snapshots.gc(1, KEEP_LONG).await;
    assert_eq!(refs(&home_git, &home).await, Vec::<String>::new(), "the turn went whole");
}
