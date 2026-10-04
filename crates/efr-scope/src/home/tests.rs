use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Home, normalize};
use crate::ScopeError;
use crate::testing::Sandbox;

#[test]
fn home_must_be_absolute_and_not_root() {
    assert!(
        matches!(Home::new("home/u"), Err(ScopeError::NotAbsolute { path }) if path == Path::new("home/u"))
    );
    assert!(matches!(Home::new("/"), Err(ScopeError::HomeIsRoot)));
    assert!(matches!(Home::new("/home/.."), Err(ScopeError::HomeIsRoot)));
}

#[test]
fn a_missing_home_keeps_its_normal_form() {
    let home = Home::new("/nonexistent/efr-test/./home/").unwrap();
    assert_eq!(home.path(), Path::new("/nonexistent/efr-test/home"));
    assert_eq!(home.canonical(), Path::new("/nonexistent/efr-test/home"));
}

#[test]
fn a_linked_home_resolves() {
    let sandbox = Sandbox::new();
    let link = sandbox.root().join("link-home");
    std::os::unix::fs::symlink(sandbox.home().path(), &link).unwrap();
    let home = Home::new(&link).unwrap();
    assert_eq!(home.path(), link);
    assert_eq!(home.canonical(), sandbox.home().path());
}

#[rstest]
#[case::home("/home/u", true)]
#[case::parent("/home", true)]
#[case::root("/", true)]
#[case::child("/home/u/p", false)]
#[case::sibling("/home/u2", false)]
#[case::elsewhere("/etc", false)]
#[case::resolved_form("/var/home/u", true)]
#[case::above_resolved_form("/var", true)]
fn at_or_above_home(#[case] dir: &str, #[case] expected: bool) {
    let home = Home::linked("/home/u", "/var/home/u");
    assert_eq!(home.is_at_or_above(Path::new(dir)), expected);
}

#[rstest]
#[case("/", Some("/"))]
#[case("/a/./b/", Some("/a/b"))]
#[case("/a/../../b", Some("/b"))]
#[case("//a//b", Some("/a/b"))]
#[case("a/b", None)]
#[case("", None)]
fn normalizes(#[case] path: &str, #[case] expected: Option<&str>) {
    assert_eq!(normalize(Path::new(path)), expected.map(PathBuf::from));
}
