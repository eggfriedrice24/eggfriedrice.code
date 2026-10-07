use pretty_assertions::assert_eq;

use super::{ChangeKind, FileChange, FileChanges, MAX_LISTED_FILES};

fn file(path: &str, added: u32, removed: u32) -> FileChange {
    FileChange {
        path: path.to_owned(),
        kind: ChangeKind::Modified,
        from: None,
        added,
        removed,
        binary: false,
    }
}

#[test]
fn from_files_sorts_counts_and_cuts_the_list() {
    let mut files: Vec<FileChange> =
        (0..MAX_LISTED_FILES + 3).map(|n| file(&format!("f{n:03}"), 2, 1)).collect();
    files.reverse();
    let changes = FileChanges::from_files(files);
    assert_eq!(changes.files.len(), MAX_LISTED_FILES);
    assert_eq!(changes.files[0].path, "f000");
    assert_eq!(changes.more, 3);
    assert_eq!(changes.count(), MAX_LISTED_FILES as u64 + 3);
    assert_eq!((changes.added, changes.removed), (106, 53));
}

#[test]
fn an_empty_list_is_empty() {
    assert!(FileChanges::from_files(Vec::new()).is_empty());
    assert!(!FileChanges::from_files(vec![file("a", 0, 0)]).is_empty());
}

#[test]
fn optional_members_are_left_out() {
    let json = serde_json::to_value(file("src/a.rs", 3, 1)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "path": "src/a.rs", "kind": "modified", "added": 3, "removed": 1 })
    );
}
