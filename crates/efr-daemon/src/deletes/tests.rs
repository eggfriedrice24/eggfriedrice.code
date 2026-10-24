//! The paths of a deleted conversation and their removal.

use std::os::unix::fs::PermissionsExt as _;

use efr_protocol::{ConversationId, PtyId, UndoId};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;

use super::{paths, remove_all};

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const PTY: &str = "019a9b1c-3d00-7a10-8b20-000000000002";
const UNDO: &str = "019a9b1c-3d00-7a10-8b20-000000000003";

#[test]
fn the_paths_of_a_conversation_are_its_own_dirs_by_id() {
    let dirs = TestDirs::new().unwrap();
    let roots = dirs.dirs();
    let id: ConversationId = CONVERSATION.parse().unwrap();
    let pty: PtyId = PTY.parse().unwrap();
    let undo: UndoId = UNDO.parse().unwrap();
    let recordings = roots.data().join("recordings");

    let found = paths(roots, &recordings, id, &[pty], &[undo]);

    let mut expected = vec![
        recordings.join(PTY),
        roots.state().join("sandbox").join(CONVERSATION),
        roots.runtime().join("sbx").join(CONVERSATION),
        roots.data().join("file-backups").join(CONVERSATION),
        roots.data().join("undo-trash").join(UNDO),
    ];
    expected.sort();
    assert_eq!(found, expected);
}

#[test]
fn removal_takes_read_only_dirs_and_keeps_the_targets_of_links() {
    let dirs = TestDirs::new().unwrap();
    let root = dirs.root();
    let gone = root.join("gone");
    let locked = gone.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("file"), b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"kept").unwrap();
    std::os::unix::fs::symlink(&outside, gone.join("link")).unwrap();
    let link = root.join("a-link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let left = remove_all(vec![gone.clone(), link.clone(), root.join("missing")]);

    assert_eq!(left, Vec::<std::path::PathBuf>::new());
    assert!(!gone.exists());
    assert!(link.symlink_metadata().is_err(), "the link went as a link");
    assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"kept");
}
