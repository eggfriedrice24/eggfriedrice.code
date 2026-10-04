use pretty_assertions::assert_eq;

use super::{FileSnapshot, JournalEntry, MemoryJournal, Original, WriteJournal as _};
use crate::testing::ids;

#[test]
fn debug_shows_the_size_of_the_content_but_not_the_content() {
    let snapshot = FileSnapshot::new(
        "/home/u/.env",
        Original::File { mode: 0o600, uid: 1000, gid: 1000, content: b"TOKEN=hunter2".to_vec() },
    );
    let debug = format!("{snapshot:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("<13 bytes>"), "{debug}");
    assert!(debug.contains("600"), "{debug}");
}

#[tokio::test]
async fn a_memory_journal_keeps_entries_in_order() {
    let journal = MemoryJournal::new();
    for path in ["/a", "/b"] {
        journal
            .record(JournalEntry::new(ids(), FileSnapshot::new(path, Original::Missing)))
            .await
            .unwrap();
    }
    let paths: Vec<_> = journal.entries().into_iter().map(|entry| entry.snapshot.path).collect();
    assert_eq!(paths, [std::path::PathBuf::from("/a"), std::path::PathBuf::from("/b")]);
}
