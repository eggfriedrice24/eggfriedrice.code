use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{git_refuses, quote, tree_path};

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
fn quotes_keep_every_byte_of_a_path() {
    assert_eq!(quote("files/a b"), "\"files/a b\"");
    assert_eq!(quote("files/\"q\"\\x"), "\"files/\\\"q\\\"\\\\x\"");
    assert_eq!(quote("files/new\nline\ttab"), "\"files/new\\nline\\011tab\"");
    assert_eq!(quote("files/é"), "\"files/é\"");
}
