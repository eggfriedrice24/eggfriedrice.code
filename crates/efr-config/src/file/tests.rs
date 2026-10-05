use std::path::PathBuf;

use pretty_assertions::assert_eq;

use crate::FileState;

#[test]
fn a_missing_file_does_not_exist_and_is_no_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let state = FileState::of(&path);

    assert_eq!(state, FileState { path: path.clone(), exists: false, symlink_target: None });
    assert_eq!(state.watched_dirs(), [dir.path().to_path_buf()]);
}

#[test]
fn a_plain_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").unwrap();

    let state = FileState::of(&path);

    assert!(state.exists);
    assert_eq!(state.symlink_target, None);
}

#[test]
fn a_link_names_its_resolved_target_and_both_directories_are_watched() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let dotfiles = dir.path().join("dotfiles/efr");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("config.toml"), "").unwrap();
    let path = config.join("config.toml");
    std::os::unix::fs::symlink("../dotfiles/efr/config.toml", &path).unwrap();

    let state = FileState::of(&path);

    let target = std::fs::canonicalize(dotfiles.join("config.toml")).unwrap();
    assert!(state.exists);
    assert_eq!(state.symlink_target.as_ref(), Some(&target));
    let target_dir = target.parent().unwrap().to_path_buf();
    assert_eq!(state.watched_dirs(), [config, target_dir]);
}

#[test]
fn a_link_to_nothing_does_not_exist_and_names_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::os::unix::fs::symlink("gone/config.toml", &path).unwrap();

    let state = FileState::of(&path);

    assert!(!state.exists);
    assert_eq!(state.symlink_target, Some(dir.path().join("gone/config.toml")));
    assert_eq!(state.watched_dirs(), [dir.path().to_path_buf(), dir.path().join("gone")]);
}

#[test]
fn a_directory_in_place_of_the_file_does_not_count_as_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::create_dir(&path).unwrap();

    assert!(!FileState::of(&path).exists);
    assert_eq!(FileState::of(&PathBuf::from("config.toml")).watched_dirs(), Vec::<PathBuf>::new());
}
