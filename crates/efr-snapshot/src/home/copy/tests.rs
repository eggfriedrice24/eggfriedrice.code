use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{quote, tree_path};

#[test]
fn a_home_path_has_one_place_in_the_tree() {
    assert_eq!(tree_path(Path::new("/home/u/.zshrc")).unwrap().unwrap(), "files/home/u/.zshrc");
    assert_eq!(tree_path(Path::new("@crontab")).unwrap().unwrap(), "state/crontab");
    let not_utf8 = Path::new(OsStr::from_bytes(b"/home/u/\xff"));
    assert_eq!(tree_path(not_utf8).unwrap(), None, "no copy holds a name that is not UTF-8");
    for bad in ["relative", "/", "/home/../etc/passwd", "/home/./u", "@", "@Cron", "@a/b"] {
        assert!(tree_path(Path::new(bad)).is_err(), "{bad}");
    }
}

#[test]
fn quotes_keep_every_byte_of_a_path() {
    assert_eq!(quote("files/a b"), "\"files/a b\"");
    assert_eq!(quote("files/\"q\"\\x"), "\"files/\\\"q\\\"\\\\x\"");
    assert_eq!(quote("files/new\nline\ttab"), "\"files/new\\nline\\011tab\"");
    assert_eq!(quote("files/é"), "\"files/é\"");
}
