use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::{CwdReport, parse};

fn report(host: Option<&str>, path: &str) -> CwdReport {
    CwdReport { host: host.map(str::to_owned), path: PathBuf::from(path) }
}

#[test]
fn file_url_with_a_host() {
    assert_eq!(parse(b"file://arch/home/egg"), Some(report(Some("arch"), "/home/egg")));
}

#[test]
fn file_url_without_a_host() {
    assert_eq!(parse(b"file:///tmp"), Some(report(None, "/tmp")));
}

#[test]
fn kitty_shell_cwd_url() {
    assert_eq!(
        parse(b"kitty-shell-cwd://arch/home/egg/src"),
        Some(report(Some("arch"), "/home/egg/src"))
    );
}

#[test]
fn file_paths_are_percent_decoded() {
    assert_eq!(
        parse(b"file://arch/home/egg/My%20Documents/caf%C3%A9"),
        Some(report(Some("arch"), "/home/egg/My Documents/caf\u{e9}"))
    );
}

#[test]
fn kitty_paths_are_raw() {
    assert_eq!(
        parse(b"kitty-shell-cwd://arch/tmp/100%/a b?#"),
        Some(report(Some("arch"), "/tmp/100%/a b?#"))
    );
}

#[test]
fn a_file_url_query_and_fragment_are_not_part_of_the_path() {
    assert_eq!(parse(b"file://h/tmp/x?y=1"), Some(report(Some("h"), "/tmp/x")));
    assert_eq!(parse(b"file://h/tmp/x#top"), Some(report(Some("h"), "/tmp/x")));
    assert_eq!(parse(b"file://h/tmp/a%23b"), Some(report(Some("h"), "/tmp/a#b")));
}

#[test]
fn the_scheme_ignores_case() {
    assert_eq!(parse(b"FILE:///tmp"), Some(report(None, "/tmp")));
    assert_eq!(parse(b"Kitty-Shell-Cwd://h/tmp"), Some(report(Some("h"), "/tmp")));
}

#[test]
fn the_root_directory() {
    assert_eq!(parse(b"file://h/"), Some(report(Some("h"), "/")));
}

#[test]
fn paths_may_hold_bytes_that_are_not_utf8() {
    let expected = PathBuf::from(OsString::from_vec(b"/tmp/\xff".to_vec()));
    assert_eq!(parse(b"file:///tmp/%FF").map(|r| r.path), Some(expected.clone()));
    assert_eq!(parse(b"kitty-shell-cwd:///tmp/\xff").map(|r| r.path), Some(expected));
}

#[test]
fn urls_that_are_not_reports() {
    assert_eq!(parse(b""), None);
    assert_eq!(parse(b"/tmp"), None);
    assert_eq!(parse(b"http://h/tmp"), None);
    assert_eq!(parse(b"file:/tmp"), None);
    assert_eq!(parse(b"file://hostonly"), None);
    assert_eq!(parse(b"file://h/bad%zz"), None);
    assert_eq!(parse(b"file://h/bad%2"), None);
    assert_eq!(parse(b"file://\xff/tmp"), None);
}
