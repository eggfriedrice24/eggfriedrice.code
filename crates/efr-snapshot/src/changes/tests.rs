use std::path::{Path, PathBuf};

use efr_protocol::{ChangeKind, FileChange};
use pretty_assertions::assert_eq;

use super::{RootChange, Shown, merge, parse, shown_prefix};

fn change(path: &str, kind: ChangeKind, added: u32, removed: u32) -> RootChange {
    RootChange { path: path.to_owned(), kind, from: None, added, removed, binary: false }
}

#[test]
fn raw_and_numstat_records_read_as_one_change_each() {
    let zero = "0000000000000000000000000000000000000000";
    let one = "8ba3a16384aacc37d01564b28401755ce8053f51";
    let out = [
        format!(":000000 100644 {zero} {one} A"),
        "added.txt".to_owned(),
        format!(":100644 100644 {one} {one} M"),
        "bin.dat".to_owned(),
        format!(":100644 000000 {one} {zero} D"),
        "gone.txt".to_owned(),
        format!(":100644 100644 {one} {one} R100"),
        "old.txt".to_owned(),
        "new.txt".to_owned(),
        format!(":160000 160000 {one} {one} M"),
        "vendor/lib".to_owned(),
        "1\t0\tadded.txt".to_owned(),
        "-\t-\tbin.dat".to_owned(),
        "0\t2\tgone.txt".to_owned(),
        "0\t0\t".to_owned(),
        "old.txt".to_owned(),
        "new.txt".to_owned(),
        "1\t1\tvendor/lib".to_owned(),
        String::new(),
    ]
    .join("\0");
    let mut renamed = change("new.txt", ChangeKind::Renamed, 0, 0);
    renamed.from = Some("old.txt".to_owned());
    let mut binary = change("bin.dat", ChangeKind::Modified, 0, 0);
    binary.binary = true;
    assert_eq!(
        parse(out.as_bytes()),
        vec![
            change("added.txt", ChangeKind::Added, 1, 0),
            binary,
            change("gone.txt", ChangeKind::Deleted, 0, 2),
            renamed,
        ]
    );
}

#[test]
fn empty_output_is_no_change() {
    assert_eq!(parse(b""), Vec::new());
}

#[test]
fn merge_shows_each_root_with_its_prefix_and_keeps_a_nested_path_once() {
    let outer = Shown {
        root: PathBuf::from("/home/u/p"),
        shown: String::new(),
        changes: vec![
            change("a.rs", ChangeKind::Modified, 2, 1),
            change("inner/x", ChangeKind::Added, 1, 0),
        ],
    };
    let inner = Shown {
        root: PathBuf::from("/home/u/p/inner"),
        shown: "~/p/inner/".to_owned(),
        changes: vec![change("x", ChangeKind::Added, 1, 0)],
    };
    let changes = merge(vec![outer, inner]).unwrap();
    let paths: Vec<&str> = changes.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["a.rs", "~/p/inner/x"]);
    assert_eq!((changes.added, changes.removed, changes.more), (3, 1, 0));
    assert_eq!(
        changes.files[0],
        FileChange {
            path: "a.rs".to_owned(),
            kind: ChangeKind::Modified,
            from: None,
            added: 2,
            removed: 1,
            binary: false,
        }
    );
}

#[test]
fn merge_of_nothing_is_none() {
    let empty = Shown { root: PathBuf::from("/p"), shown: String::new(), changes: Vec::new() };
    assert_eq!(merge(vec![empty]), None);
}

#[test]
fn a_root_shows_as_the_project_scratch_home_or_absolute() {
    let home = Path::new("/home/u");
    let project = Path::new("/home/u/p/app");
    let scratch = Path::new("/home/u/.local/share/efr/scratch/s1");
    let prefix = |root: &str| shown_prefix(Path::new(root), Some(project), Some(scratch), home);
    assert_eq!(prefix("/home/u/p/app"), "");
    assert_eq!(prefix("/home/u/.local/share/efr/scratch/s1"), "$SCRATCH/");
    assert_eq!(prefix("/home/u/p/other"), "~/p/other/");
    assert_eq!(prefix("/home/u"), "~/");
    assert_eq!(prefix("/etc/nixos"), "/etc/nixos/");
    assert_eq!(prefix("/"), "/");
}
