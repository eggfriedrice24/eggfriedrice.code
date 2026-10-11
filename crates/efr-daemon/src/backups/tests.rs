use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use efr_protocol::{ConversationId, TurnId};
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use crate::backups::collect_at;

fn root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

#[test]
fn legacy_file_backups_are_collected() {
    let (_temp, root) = root();
    let store = root.join("file-backups");
    let clock = TestClock::new();
    let rng = TestRng::new(7);
    let day = Duration::from_secs(24 * 3600);
    let id = || efr_stdx::id::uuid_v7(&clock, &rng);
    // Conversation a: four turns one day apart; conversation b: one turn of the first
    // day.
    let a = ConversationId::from_uuid(id());
    let b = ConversationId::from_uuid(id());
    let mut turns_a = Vec::new();
    let mut turn_b = None;
    for n in 0..4 {
        let turn = TurnId::from_uuid(id());
        turns_a.push(turn);
        if n == 0 {
            turn_b = Some(TurnId::from_uuid(id()));
        }
        clock.advance(day);
    }
    let turn_b = turn_b.unwrap();
    let entry = |conversation: ConversationId, turn: TurnId| {
        let dir = store.join(conversation.to_string()).join(turn.to_string()).join("1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".zshrc"), "old\n").unwrap();
        dir.parent().unwrap().to_path_buf()
    };
    let dirs_a: Vec<PathBuf> = turns_a.iter().map(|turn| entry(a, *turn)).collect();
    let dir_b = entry(b, turn_b);
    std::fs::create_dir_all(store.join("not-an-id")).unwrap();
    let now = SystemTime::from(efr_stdx::time::Clock::now(&clock));

    // Keep two turns per conversation, of at most ten days.
    let removed = collect_at(&store, 2, 10 * day, now);

    assert_eq!(removed, 2);
    assert!(!dirs_a[0].exists() && !dirs_a[1].exists(), "the oldest turns of a go");
    assert!(dirs_a[2].exists() && dirs_a[3].exists(), "the newest two stay");
    assert!(dir_b.exists(), "each conversation keeps its own newest turns");

    // Three days later, with at most four days: b's turn of the first day is too old.
    let removed = collect_at(&store, 2, 4 * day, now + 3 * day);

    assert_eq!(removed, 2, "b's turn and a's turn of the second day");
    assert!(!store.join(b.to_string()).exists(), "an empty conversation dir goes");
    assert!(dirs_a[3].exists());
    assert!(store.join("not-an-id").exists(), "a name that is not efr's stays");

    std::fs::remove_dir(store.join("not-an-id")).unwrap();
    collect_at(&store, 0, 4 * day, now + 3 * day);
    assert!(!store.exists(), "the store goes once it is empty");
}
