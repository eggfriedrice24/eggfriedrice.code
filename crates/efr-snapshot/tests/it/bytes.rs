//! Each blob holds the exact bytes of its file, whatever the root's `.gitattributes`
//! asks git to do, also in a store from before the store's own `info/attributes`.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use pretty_assertions::assert_eq;

use crate::support::{
    World, blobs_of, conversation, git, index_blobs, limits, root, store, turn, write,
};

/// The id that git gives the bytes of `file` with no filter and no conversion.
async fn raw_id(home: &Path, file: &Path) -> String {
    let path = file.to_str().unwrap();
    git(home, home, &[], &["hash-object", "--no-filters", path]).await.trim().to_owned()
}

/// Files whose bytes an in-tree `.gitattributes` would change on `git add`.
fn plant(dir: &Path) {
    write(
        &dir.join(".gitattributes"),
        "a.txt text eol=crlf\nb.txt working-tree-encoding=UTF-16\nc.txt ident\n\
         d.bin filter=lfs\n* text=auto\n",
    );
    fs::write(dir.join("a.txt"), b"line1\r\nline2\n").unwrap();
    fs::write(dir.join("b.txt"), b"\xff\xfeh\x00i\x00\n\x00").unwrap();
    fs::write(dir.join("c.txt"), b"$Id: deadbeef $\n$Id$\n").unwrap();
    fs::write(dir.join("d.bin"), b"version https://x\r\n").unwrap();
    fs::write(dir.join("e.txt"), b"mixed\r\nends\n").unwrap();
}

const PLANTED: &[&str] = &[".gitattributes", "a.txt", "b.txt", "c.txt", "d.bin", "e.txt"];

#[tokio::test]
async fn snapshot_is_byte_exact_with_gitattributes() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("attrs");
    plant(&dir);
    git(&dir, &home, &[], &["init", "-q"]).await;

    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await;

    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(1));
    let blobs = blobs_of(&world.only_store(), &home, &pre).await;
    for name in PLANTED {
        assert_eq!(blobs[*name], raw_id(&home, &dir.join(name)).await, "{name}");
    }
}

#[tokio::test]
async fn old_store_index_is_rebuilt_for_exact_bytes() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("old");
    plant(&dir);
    let call =
        world.snapshots.before_call(conversation(1), turn(1), vec![root(&dir)], limits()).await;
    world.snapshots.after_call(call, limits()).await;
    world.snapshots.finish_turn(turn(1), limits()).await;

    // NOTE: a store from before phase B: no info/attributes, no format mark, and an
    // index whose stat cache holds the blobs that the attributes changed.
    let git_dir = world.only_store();
    let index = git_dir.with_extension("index");
    fs::remove_file(git_dir.join("info/attributes")).unwrap();
    fs::remove_file(&index).unwrap();
    let env = [("GIT_DIR", git_dir.as_path())];
    git(&home, &home, &env, &["config", "--unset", "efr.format"]).await;
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for name in PLANTED {
        fs::File::options().write(true).open(dir.join(name)).unwrap().set_modified(old).unwrap();
    }
    let env = [
        ("GIT_DIR", git_dir.as_path()),
        ("GIT_INDEX_FILE", index.as_path()),
        ("GIT_WORK_TREE", dir.as_path()),
    ];
    git(&dir, &home, &env, &["add", "-A"]).await;
    let normalized = index_blobs(&git_dir, &home).await;
    assert_ne!(
        normalized["a.txt"],
        raw_id(&home, &dir.join("a.txt")).await,
        "the old store normalized"
    );

    // A new efrd process: its first snapshot of the root rebuilds the index.
    let snapshots = store(world.base.path(), &world.clock);
    let call = snapshots.before_call(conversation(1), turn(2), vec![root(&dir)], limits()).await;
    snapshots.after_call(call, limits()).await;
    snapshots.finish_turn(turn(2), limits()).await;
    let pre = format!("refs/efr/{}/{}/pre", conversation(1), turn(2));
    let blobs = blobs_of(&git_dir, &home, &pre).await;
    for name in PLANTED {
        assert_eq!(blobs[*name], raw_id(&home, &dir.join(name)).await, "{name}");
    }
    let mark =
        git(&home, &home, &[("GIT_DIR", git_dir.as_path())], &["config", "efr.format"]).await;
    assert_eq!(mark.trim(), "2");
}
