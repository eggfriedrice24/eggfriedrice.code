use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use efr_protocol::{ConversationId, TurnId};
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use crate::backups::{MAX_BACKUP_BYTES, META_FILE, Meta, Target, collect_at, finish, take};

fn ids() -> (ConversationId, TurnId) {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    (ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2)))
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

fn floor(path: &Path) -> Target {
    Target { path: path.to_path_buf(), floor: true, link: None }
}

fn meta(entry: &Path) -> Meta {
    serde_json::from_slice(&std::fs::read(entry.join(META_FILE)).unwrap()).unwrap()
}

#[test]
fn a_copy_keeps_the_content_the_mode_and_the_links_to_the_real_file() {
    let (_temp, root) = root();
    let home = root.join("home");
    let real = home.join("dotfiles/zsh/.zshrc");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "alias ll='ls -l'\n").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink("dotfiles/zsh/.zshrc", home.join(".zshrc")).unwrap();
    let store = root.join("data/file-backups");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let (conversation, turn) = ids();

    let taken = take(&store, conversation, turn, &[floor(&home.join(".zshrc"))], &[]).unwrap();

    let entry = store.join(conversation.to_string()).join(turn.to_string()).join("1");
    assert_eq!(std::fs::read_to_string(entry.join(".zshrc")).unwrap(), "alias ll='ls -l'\n");
    assert_eq!(
        meta(&entry),
        Meta {
            path: home.join(".zshrc"),
            links: vec![home.join(".zshrc")],
            real: real.clone(),
            mode: Some(0o640),
            missing: false,
        }
    );
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    for dir in [store.clone(), entry.parent().unwrap().to_path_buf(), entry.clone()] {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode(&entry.join(".zshrc")), 0o600);
    assert_eq!(mode(&entry.join(META_FILE)), 0o600);

    std::fs::write(&real, "alias ll='ls -la'\n").unwrap();
    let backups = finish(taken);
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].path, home.join(".zshrc"));
    assert_eq!(backups[0].backup, entry.join(".zshrc"));
    assert!(backups[0].floor);
}

#[test]
fn two_names_of_one_file_take_one_copy_and_the_next_call_takes_the_next_entry() {
    let (_temp, root) = root();
    let real = root.join("p/dots/hypr/hyprland.conf");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "gaps_in = 5\n").unwrap();
    std::fs::create_dir_all(root.join("home/.config")).unwrap();
    std::os::unix::fs::symlink(real.parent().unwrap(), root.join("home/.config/hypr")).unwrap();
    let store = root.join("file-backups");
    let (conversation, turn) = ids();
    let linked = root.join("home/.config/hypr/hyprland.conf");

    let first = take(&store, conversation, turn, &[floor(&linked), floor(&real)], &[]).unwrap();
    let second = take(&store, conversation, turn, &[floor(&real)], &[]).unwrap();

    assert_eq!(first.len(), 1);
    let turn_dir = store.join(conversation.to_string()).join(turn.to_string());
    assert_eq!(meta(&turn_dir.join("1")).links, [root.join("home/.config/hypr")]);
    assert_eq!(meta(&turn_dir.join("2")).path, real);
    std::fs::write(&real, "gaps_in = 10\n").unwrap();
    assert_eq!(finish(first).len(), 1);
    assert_eq!(finish(second).len(), 1);
}

/// The live run (case 5.6): the turn's line named `~/dotfiles/hypr/efr-test.conf`,
/// the real file of a new file under `~/.config/hypr`, a link to `~/dotfiles/hypr`.
/// The copy and the line keep the path through the link and name the real file too.
#[test]
fn a_new_file_under_a_dir_link_is_named_by_the_link() {
    let (_temp, root) = root();
    let home = root.join("home");
    let dots = home.join("dotfiles/hypr");
    std::fs::create_dir_all(&dots).unwrap();
    std::fs::create_dir_all(home.join(".config")).unwrap();
    std::os::unix::fs::symlink(&dots, home.join(".config/hypr")).unwrap();
    let store = root.join("data/file-backups");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let (conversation, turn) = ids();
    let real = dots.join("efr-test.conf");
    let linked = home.join(".config/hypr/efr-test.conf");
    let target = Target { path: real.clone(), floor: true, link: Some(linked.clone()) };

    let taken = take(&store, conversation, turn, &[target], &[]).unwrap();

    let entry = store.join(conversation.to_string()).join(turn.to_string()).join("1");
    assert_eq!(
        meta(&entry),
        Meta {
            path: linked.clone(),
            links: vec![home.join(".config/hypr")],
            real: real.clone(),
            mode: None,
            missing: true,
        }
    );
    std::fs::write(&real, "# efr\n").unwrap();
    let backups = finish(taken);
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].path, linked);
    assert_eq!(backups[0].real, Some(real.clone()));
    assert_eq!(backups[0].backup, entry.join(META_FILE));

    // A link that no longer leads to the file does not name it.
    let elsewhere = home.join(".config/other.conf");
    let target = Target { path: real.clone(), floor: true, link: Some(elsewhere) };
    let taken = take(&store, conversation, turn, &[target], &[]).unwrap();
    std::fs::write(&real, "# efr again\n").unwrap();
    let backups = finish(taken);
    assert_eq!(backups[0].path, real);
    assert_eq!(backups[0].real, None);
}

#[test]
fn a_directory_or_a_fifo_takes_no_copy() {
    let (_temp, root) = root();
    std::fs::create_dir_all(root.join("home/.config/autostart")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        root.join("home/.zshrc"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let store = root.join("file-backups");
    let (conversation, turn) = ids();
    let targets = [floor(&root.join("home/.config/autostart")), floor(&root.join("home/.zshrc"))];

    let taken = take(&store, conversation, turn, &targets, &[]).unwrap();

    assert!(taken.is_empty());
    assert!(!store.join(conversation.to_string()).exists());
}

#[test]
fn a_file_over_the_limit_stops_the_call_and_leaves_no_entry() {
    let (_temp, root) = root();
    let small = root.join("home/.bashrc");
    let large = root.join("home/.local/bin/tool");
    std::fs::create_dir_all(large.parent().unwrap()).unwrap();
    std::fs::write(&small, "x").unwrap();
    std::fs::write(&large, vec![0_u8; MAX_BACKUP_BYTES + 1]).unwrap();
    let store = root.join("file-backups");
    let (conversation, turn) = ids();

    let refused = take(&store, conversation, turn, &[floor(&small), floor(&large)], &[]);

    let reason = refused.unwrap_err();
    assert!(reason.contains("the call did not run"), "{reason}");
    assert!(reason.contains("larger than 16 MiB"), "{reason}");
    assert!(!store.join(conversation.to_string()).exists(), "the copy of .bashrc went too");
}

#[test]
fn floor_backups_collected_with_snapshot_keys() {
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
}
