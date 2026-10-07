use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use efr_stdx::env::Var;
use pretty_assertions::assert_eq;

use super::TestDirs;
use crate::TestSupportError;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_tree_holds_four_private_roots_and_a_home() {
    let dirs = TestDirs::new().unwrap();
    let root = dirs.root();
    let efr = dirs.dirs();
    assert_eq!(efr.config(), root.join("config"));
    assert_eq!(efr.data(), root.join("data"));
    assert_eq!(efr.state(), root.join("state"));
    assert_eq!(efr.runtime(), root.join("runtime"));
    assert_eq!(dirs.home(), root.join("home"));
    for dir in [efr.config(), efr.data(), efr.state(), efr.runtime(), dirs.home()] {
        assert!(dir.is_dir(), "{} is missing", dir.display());
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
}

#[test]
fn the_root_is_a_real_absolute_path() {
    let dirs = TestDirs::new().unwrap();
    assert!(dirs.root().is_absolute());
    assert_eq!(dirs.root().canonicalize().unwrap(), dirs.root());
    assert!(dirs.root().file_name().unwrap().to_str().unwrap().starts_with("efr-test-"));
}

#[test]
fn two_trees_do_not_share_a_root() {
    let one = TestDirs::new().unwrap();
    let two = TestDirs::new().unwrap();
    assert_ne!(one.root(), two.root());
}

#[test]
fn dropping_the_value_removes_the_tree() {
    let dirs = TestDirs::new().unwrap();
    let root = dirs.root().to_path_buf();
    drop(dirs);
    assert!(!root.exists());
}

#[test]
fn the_environment_names_the_same_roots() {
    let dirs = TestDirs::new().unwrap();
    let env = dirs.env();
    let efr = dirs.dirs();
    assert_eq!(env.path(Var::ConfigDir).unwrap().as_deref(), Some(efr.config()));
    assert_eq!(env.path(Var::DataDir).unwrap().as_deref(), Some(efr.data()));
    assert_eq!(env.path(Var::StateDir).unwrap().as_deref(), Some(efr.state()));
    assert_eq!(env.path(Var::RuntimeDir).unwrap().as_deref(), Some(efr.runtime()));
    assert_eq!(env.var(Var::Log).unwrap(), None);
}

#[test]
fn create_dir_makes_nested_directories_under_the_root() {
    let dirs = TestDirs::new().unwrap();
    let project = dirs.create_dir("home/src/project").unwrap();
    assert_eq!(project, dirs.root().join("home/src/project"));
    assert!(project.is_dir());
    assert_eq!(mode(&project), 0o700);
    assert_eq!(dirs.create_dir("home/src/project").unwrap(), project);
}

#[test]
fn the_redactor_hides_the_root_and_keeps_room_for_a_cwd() {
    let dirs = TestDirs::new().unwrap();
    let project = dirs.create_dir("home/project").unwrap();
    let socket = dirs.dirs().socket_path();
    let redactor = dirs.redactor().cwd(&project);
    let text = format!("{} {}", project.display(), socket.display());
    assert_eq!(redactor.redact(&text), "<CWD> <TMP>/runtime/daemon.sock");
}

#[test]
fn create_dir_refuses_paths_that_leave_the_tree() {
    let dirs = TestDirs::new().unwrap();
    for path in ["/tmp/efr-escape", "home/../../efr-escape", ".."] {
        let err = dirs.create_dir(path).unwrap_err();
        assert!(
            matches!(&err, TestSupportError::OutsideTree { path: given } if given == Path::new(path)),
            "{path}: {err:?}"
        );
    }
    assert!(!dirs.root().parent().unwrap().join("efr-escape").exists());
}

#[test]
fn a_tree_can_live_below_another_directory() {
    let base = tempfile::tempdir().unwrap();
    let dirs = TestDirs::new_in(&base.path().join("deeper")).unwrap();
    assert!(dirs.root().starts_with(base.path().canonicalize().unwrap()));
    assert!(dirs.dirs().runtime().is_dir() && dirs.home().is_dir());
}
