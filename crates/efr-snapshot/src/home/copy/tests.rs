use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::UndoReason;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::{CommitPlan, Item, Kind, Manifest, git_refuses, produce, quote, tree_path};
use crate::home::{HomeEntries, HomeEntry, HomeState};

#[test]
fn a_home_path_has_one_place_in_the_tree() {
    let place = |path: &str| tree_path(Path::new(path), false).unwrap().unwrap();
    assert_eq!(place("/home/u/.zshrc"), "files/home/u/.zshrc");
    assert_eq!(place("@crontab"), "state/crontab");
    let not_utf8 = Path::new(OsStr::from_bytes(b"/home/u/\xff"));
    assert_eq!(tree_path(not_utf8, false).unwrap(), None, "no copy holds a name that is not UTF-8");
    for bad in ["relative", "/", "/home/../etc/passwd", "/home/./u", "@", "@Cron", "@a/b"] {
        assert!(tree_path(Path::new(bad), false).is_err(), "{bad}");
    }
}

#[test]
fn no_copy_holds_a_name_that_git_refuses() {
    for refused in [".git", ".GIT", ".Git", "git~1", "GIT~1", ".git.", ".git ", ".git::$DATA"] {
        assert!(git_refuses(OsStr::new(refused), false), "{refused}");
        let path = format!("/home/u/notes/{refused}/HEAD");
        assert_eq!(tree_path(Path::new(&path), false).unwrap(), None, "{path}");
    }
    for kept in [".gitx", ".git.d", "a.git", "git~2", ".gitignore", ".gitmodules", "gitmod~1"] {
        assert!(!git_refuses(OsStr::new(kept), false), "{kept}");
    }
    for link in [".gitmodules", ".GITMODULES", ".gitmodules.", "gitmod~1", "gi7eba~1"] {
        assert!(git_refuses(OsStr::new(link), true), "a link named {link}");
    }
    assert!(!git_refuses(OsStr::new(".gitignore"), true));
    assert_eq!(
        tree_path(Path::new("/home/u/.gitmodules"), false).unwrap().as_deref(),
        Some("files/home/u/.gitmodules")
    );
    assert_eq!(tree_path(Path::new("/home/u/.gitmodules"), true).unwrap(), None);
}

#[test]
fn old_home_copies_still_read() {
    // NOTE: a manifest as efr wrote it before the missing parents.
    let old = r#"{"phase":"before","entries":[{"path":"/home/u/.zshrc","kind":"file","mode":420,"blob":"0123456789abcdef0123456789abcdef01234567","floor":true,"chain":[],"reason":null},{"path":"/home/u/.config/systemd/user/default.target.wants/x.service","kind":"missing","mode":null,"blob":null,"floor":true,"chain":[],"reason":null},{"path":"/home/u/.ssh/id","kind":"not_covered","mode":null,"blob":null,"floor":false,"chain":[],"reason":"masked"}]}"#;
    let manifest: Manifest = serde_json::from_str(old).unwrap();
    assert_eq!(manifest.phase, "before");
    let kinds: Vec<(Kind, Vec<String>)> =
        manifest.entries.iter().map(|item| (item.kind, item.parents.clone())).collect();
    assert_eq!(
        kinds,
        [(Kind::File, Vec::new()), (Kind::Missing, Vec::new()), (Kind::NotCovered, Vec::new())]
    );
    assert_eq!(manifest.entries[2].reason, Some(UndoReason::Masked));
    let new = Item {
        path: "/home/u/a/b/c".to_owned(),
        parents: vec!["/home/u/a/b".to_owned(), "/home/u/a".to_owned()],
        ..Item::default()
    };
    let text = serde_json::to_string(&Manifest { phase: "before".to_owned(), entries: vec![new] });
    let text = text.unwrap();
    assert!(text.contains(r#""parents":["/home/u/a/b","/home/u/a"]"#), "{text}");
    let plain = serde_json::to_string(&Item::default()).unwrap();
    assert!(!plain.contains("parents"), "a path with no missing parent writes none: {plain}");
}

#[test]
fn the_index_of_a_manifest_keeps_the_first_item_of_a_path() {
    let item = |path: &str, kind: Kind| Item { path: path.to_owned(), kind, ..Item::default() };
    let manifest = Manifest {
        phase: "before".to_owned(),
        entries: vec![
            item("/home/u/a", Kind::File),
            item("/home/u/b", Kind::Missing),
            item("/home/u/a", Kind::NotCovered),
        ],
    };
    let index = manifest.index();
    assert_eq!(index.len(), 2);
    assert_eq!(index["/home/u/a"].kind, Kind::File, "the first item of the path");
    assert_eq!(index["/home/u/b"].kind, Kind::Missing);
    assert!(!index.contains_key("/home/u/c"));
}

#[test]
fn a_copy_that_names_a_path_twice_keeps_the_first() {
    let entry = |path: &str, state: HomeState| HomeEntry::new(PathBuf::from(path), state, false);
    let mut reader = HomeEntries::new(vec![
        (entry("/home/u/a", HomeState::File { mode: 0o644 }), Some(b"first\n".to_vec())),
        (entry("/home/u/b", HomeState::Missing), None),
        (entry("/home/u/a", HomeState::Missing), None),
        (entry("/home/u/c", HomeState::NotCovered(UndoReason::Masked)), None),
        (entry("/home/u/b", HomeState::File { mode: 0o600 }), Some(b"second\n".to_vec())),
    ]);
    let plan = CommitPlan {
        name: "refs/efr/x/home/1".to_owned(),
        from: None,
        message: "copy\n".to_owned(),
        time: "0 +0000".to_owned(),
        phase: "before".to_owned(),
    };
    let (sender, mut parts) = mpsc::channel(4);
    let drain = std::thread::spawn(move || while parts.blocking_recv().is_some() {});
    let items = produce(&mut reader, &plan, sender).unwrap();
    drain.join().unwrap();
    let kinds: Vec<(&str, Kind)> =
        items.iter().map(|item| (item.path.as_str(), item.kind)).collect();
    assert_eq!(
        kinds,
        [("/home/u/a", Kind::File), ("/home/u/b", Kind::Missing), ("/home/u/c", Kind::NotCovered)],
        "the first entry of each path, in the order of the reader"
    );
}

#[test]
fn quotes_keep_every_byte_of_a_path() {
    assert_eq!(quote("files/a b"), "\"files/a b\"");
    assert_eq!(quote("files/\"q\"\\x"), "\"files/\\\"q\\\"\\\\x\"");
    assert_eq!(quote("files/new\nline\ttab"), "\"files/new\\nline\\011tab\"");
    assert_eq!(quote("files/é"), "\"files/é\"");
}
