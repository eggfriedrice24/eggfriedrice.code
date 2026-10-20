use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use efr_protocol::ProjectId;
use efr_scope::Registry;
use pretty_assertions::assert_eq;

use super::{path, sync, text};

fn project(n: u128) -> ProjectId {
    ProjectId::from_uuid(uuid::Uuid::from_u128(n))
}

#[test]
fn each_project_is_one_line_with_its_root_and_name_and_the_deepest_root_first() {
    let mut registry = Registry::empty();
    registry.register(project(1), PathBuf::from("/home/u/p/app"), Some("app".to_owned())).unwrap();
    registry.register(project(2), PathBuf::from("/home/u/p/app/vendor/lib"), None).unwrap();
    registry
        .register(project(3), PathBuf::from("/home/u/dots"), Some("dotfiles".to_owned()))
        .unwrap();

    let text = String::from_utf8(text(&registry)).unwrap();

    assert_eq!(text, "/home/u/p/app/vendor/lib\tlib\n/home/u/p/app\tapp\n/home/u/dots\tdotfiles\n");
}

#[test]
fn a_root_with_a_tab_is_left_out_and_a_tab_in_a_name_is_a_blank() {
    let mut registry = Registry::empty();
    registry.register(project(1), PathBuf::from("/home/u/p/a\tb"), None).unwrap();
    registry.register(project(2), PathBuf::from("/home/u/p/c\nd"), None).unwrap();
    registry
        .register(project(3), PathBuf::from("/home/u/p/app"), Some("my\tapp\nx".to_owned()))
        .unwrap();

    assert_eq!(String::from_utf8(text(&registry)).unwrap(), "/home/u/p/app\tmy app x\n");
}

#[test]
fn an_empty_registry_is_an_empty_file() {
    assert!(text(&Registry::empty()).is_empty());
}

#[test]
fn sync_writes_a_private_file_once_and_again_only_after_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let file = path(dir.path());

    assert!(sync(&file, b"/home/u/p/app\tapp\n").unwrap());
    assert!(!sync(&file, b"/home/u/p/app\tapp\n").unwrap(), "the same text is not written");
    assert!(sync(&file, b"").unwrap());

    assert_eq!(std::fs::read(&file).unwrap(), b"");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}
