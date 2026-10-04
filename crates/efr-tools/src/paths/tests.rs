use std::os::unix::fs::symlink;
use std::path::PathBuf;

use efr_scope::Home;
use pretty_assertions::assert_eq;

use super::{check_real, resolve};
use crate::ToolError;
use crate::testing::Fixture;

#[test]
fn paths_resolve_against_home_and_cwd() {
    let fixture = Fixture::new();
    let ctx = fixture.context();
    let home = fixture.home();
    let cwd = fixture.cwd();
    assert_eq!(resolve(&ctx, "~").unwrap(), home);
    assert_eq!(resolve(&ctx, "~/.zshrc").unwrap(), home.join(".zshrc"));
    assert_eq!(resolve(&ctx, "notes.txt").unwrap(), cwd.join("notes.txt"));
    assert_eq!(resolve(&ctx, "/etc/./hosts").unwrap(), PathBuf::from("/etc/hosts"));
    assert_eq!(resolve(&ctx, "../x").unwrap(), cwd.parent().unwrap().join("x"));
    assert_eq!(resolve(&ctx, "/../../etc//passwd").unwrap(), PathBuf::from("/etc/passwd"));
}

#[test]
fn a_tilde_user_path_is_a_plain_relative_name() {
    let fixture = Fixture::new();
    assert_eq!(resolve(&fixture.context(), "~root/x").unwrap(), fixture.cwd().join("~root/x"));
}

#[test]
fn an_empty_path_is_refused() {
    let fixture = Fixture::new();
    assert!(matches!(resolve(&fixture.context(), ""), Err(ToolError::EmptyPath)));
}

#[test]
fn real_paths_and_paths_that_do_not_exist_yet_pass() {
    let fixture = Fixture::new();
    let home = Home::new(fixture.home()).unwrap();
    std::fs::write(fixture.cwd().join("file"), "x").unwrap();
    check_real(&home, &fixture.cwd().join("file")).unwrap();
    check_real(&home, &fixture.cwd().join("new/dir/file")).unwrap();
}

#[test]
fn a_symlink_in_the_path_is_refused_with_the_real_path() {
    let fixture = Fixture::new();
    let home = Home::new(fixture.home()).unwrap();
    let secrets = fixture.root().join("secrets");
    std::fs::create_dir(&secrets).unwrap();
    symlink(&secrets, fixture.cwd().join("link")).unwrap();
    let error = check_real(&home, &fixture.cwd().join("link/key")).unwrap_err();
    let ToolError::ThroughSymlink { real, .. } = error else {
        panic!("expected a symlink error, got {error:?}");
    };
    assert_eq!(real, secrets.join("key"));
}

#[test]
fn a_symlinked_file_is_refused() {
    let fixture = Fixture::new();
    let home = Home::new(fixture.home()).unwrap();
    std::fs::write(fixture.cwd().join("target"), "x").unwrap();
    symlink(fixture.cwd().join("target"), fixture.cwd().join("alias")).unwrap();
    assert!(matches!(
        check_real(&home, &fixture.cwd().join("alias")),
        Err(ToolError::ThroughSymlink { .. })
    ));
}

#[test]
fn a_home_reached_through_a_link_is_not_a_symlink_problem() {
    let fixture = Fixture::new();
    let real_home = fixture.home();
    let linked_home = fixture.root().join("linked-home");
    symlink(&real_home, &linked_home).unwrap();
    let home = Home::new(&linked_home).unwrap();
    std::fs::write(real_home.join(".zshrc"), "").unwrap();
    check_real(&home, &linked_home.join(".zshrc")).unwrap();
}
