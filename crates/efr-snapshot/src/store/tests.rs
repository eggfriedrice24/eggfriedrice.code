use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{Store, project_head, root_id};

#[test]
fn a_root_id_is_16_hex_characters_of_its_path() {
    let id = root_id(Path::new("/home/u/p/app"));
    assert_eq!(id.len(), 16);
    assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(id, root_id(Path::new("/home/u/p/app")));
    assert_ne!(id, root_id(Path::new("/home/u/p/app2")));
}

#[test]
fn the_store_files_sit_side_by_side() {
    let store = Store::new(Path::new("/d/snapshots"), Path::new("/p"));
    let id = root_id(Path::new("/p"));
    assert_eq!(store.git_dir(), Path::new("/d/snapshots").join(format!("{id}.git")));
    assert_eq!(store.index(), Path::new("/d/snapshots").join(format!("{id}.index")));
    assert_eq!(store.root_file(), Path::new("/d/snapshots").join(format!("{id}.root")));
    assert_eq!(store.index_entries(), None);
}

#[test]
fn the_project_exclude_is_copied_but_never_through_a_link() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("p");
    fs::create_dir_all(root.join(".git/info")).unwrap();
    fs::write(root.join(".git/info/exclude"), "local-notes\n").unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let store = Store::new(&base.path().join("store"), &root);
    fs::create_dir_all(store.git_dir().join("info")).unwrap();
    store.copy_exclude().unwrap();
    let copied = store.git_dir().join("info/exclude");
    assert_eq!(fs::read_to_string(&copied).unwrap(), "local-notes\n");
    assert_eq!(project_head(&root).as_deref(), Some("ref: refs/heads/main"));

    fs::write(base.path().join("secret"), "not an exclude\n").unwrap();
    fs::remove_file(root.join(".git/info/exclude")).unwrap();
    symlink(base.path().join("secret"), root.join(".git/info/exclude")).unwrap();
    store.copy_exclude().unwrap();
    assert_eq!(fs::read_to_string(&copied).unwrap(), "", "a link is not followed");
}

#[test]
fn a_linked_work_tree_reads_the_exclude_of_its_common_dir() {
    let base = tempfile::tempdir().unwrap();
    let common = base.path().join("main/.git");
    let linked = common.join("worktrees/wt");
    fs::create_dir_all(common.join("info")).unwrap();
    fs::create_dir_all(&linked).unwrap();
    fs::write(common.join("info/exclude"), "shared\n").unwrap();
    fs::write(linked.join("commondir"), "../..\n").unwrap();
    let root = base.path().join("wt");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".git"), format!("gitdir: {}\n", linked.display())).unwrap();
    let store = Store::new(&base.path().join("store"), &root);
    fs::create_dir_all(store.git_dir().join("info")).unwrap();
    store.copy_exclude().unwrap();
    assert_eq!(fs::read_to_string(store.git_dir().join("info/exclude")).unwrap(), "shared\n");
}
