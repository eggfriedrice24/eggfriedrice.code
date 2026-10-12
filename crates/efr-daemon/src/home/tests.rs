use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use efr_permissions::{CoverAsk, Coverage};
use efr_protocol::{ConversationId, TurnId, UndoReason};
use efr_scope::{Git, Home};
use efr_snapshot::{DEFAULT_SNAPSHOT_TIMEOUT, HomeState, SnapshotParts, Snapshots};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;

use crate::home::{
    HomeLimits, Prepared, Reader, Target, after, changed, collect_trash, coverage, inside, prepare,
};

fn root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

const LIMITS: HomeLimits = HomeLimits { max_files: 2000, max_bytes: 256 * 1024 * 1024 };

fn file(path: &Path) -> Target {
    Target { ask: CoverAsk::Home(path.to_path_buf()), floor: false, link: None }
}

fn floor(path: &Path) -> Target {
    Target { ask: CoverAsk::Home(path.to_path_buf()), floor: true, link: None }
}

fn dir(path: &Path) -> Target {
    Target { ask: CoverAsk::HomeDir(path.to_path_buf()), floor: false, link: None }
}

/// The state of each entry of `prepared`, by path.
fn states(prepared: &Prepared) -> Vec<(PathBuf, HomeState)> {
    prepared.entries.iter().map(|entry| (entry.path.clone(), entry.state.clone())).collect()
}

/// The mode of each file that [`write`] makes.
const MODE: u32 = 0o644;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(MODE)).unwrap();
}

fn store(root: &Path) -> Snapshots {
    let clock = TestClock::new();
    Snapshots::new(SnapshotParts {
        dir: root.join("data/snapshots"),
        git: Git::new(clock.shared()).isolated(),
        home: Home::new(root.join("home")).unwrap(),
        clock: clock.shared(),
        excludes_file: None,
        timeout: DEFAULT_SNAPSHOT_TIMEOUT,
    })
}

fn ids() -> (ConversationId, TurnId) {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    (ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2)))
}

#[test]
fn home_copy_records_missing_path() {
    let (_temp, root) = root();
    let conf = root.join("home/.config/hypr/monitors.conf");

    let prepared = prepare(&[floor(&conf)], &[], LIMITS).unwrap();

    assert_eq!(states(&prepared), [(conf, HomeState::Missing)]);
    assert!(prepared.not_covered.is_empty());
    assert!(prepared.covers(&CoverAsk::Home(root.join("home/.config/hypr/monitors.conf"))));
}

#[test]
fn home_copy_never_follows_symlink() {
    let (_temp, root) = root();
    let home = root.join("home");
    let real = home.join("dotfiles/zsh/.zshrc");
    write(&real, "old\n");
    std::os::unix::fs::symlink(&real, home.join(".zshrc")).unwrap();
    let outside = root.join("outside");
    write(&outside.join("secret.txt"), "not mine to read\n");
    std::fs::create_dir_all(home.join("notes")).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("notes/elsewhere")).unwrap();

    let linked = prepare(&[floor(&home.join(".zshrc"))], &[], LIMITS).unwrap();
    let walked = prepare(&[dir(&home.join("notes"))], &[], LIMITS).unwrap();

    assert_eq!(
        states(&linked),
        [
            (
                home.join(".zshrc"),
                HomeState::Link { target: real.as_os_str().as_encoded_bytes().to_vec() }
            ),
            (real.clone(), HomeState::File { mode: MODE }),
        ]
    );
    assert_eq!(linked.entries[1].chain, [home.join(".zshrc")], "the links on the way");
    assert_eq!(linked.shown, [(real, home.join(".zshrc"))]);
    let target = outside.as_os_str().as_encoded_bytes().to_vec();
    assert_eq!(
        states(&walked),
        [(home.join("notes/elsewhere"), HomeState::Link { target })],
        "a link in a walked dir stays a link, and the walk never goes through it"
    );
}

#[test]
fn home_copy_skips_masks_and_secrets() {
    let (_temp, root) = root();
    let home = root.join("home");
    write(&home.join(".aws/config"), "[default]\n");
    write(&home.join("work/notes.md"), "notes\n");
    write(&home.join("work/.ssh/id_ed25519"), "KEY\n");
    let masks = [home.join(".aws"), home.join("work/.ssh")];

    let named = prepare(&[file(&home.join(".aws/config"))], &masks, LIMITS).unwrap();
    let walked = prepare(&[dir(&home.join("work"))], &masks, LIMITS).unwrap();
    let floor = prepare(&[floor(&home.join(".aws/config"))], &masks, LIMITS);

    assert_eq!(
        states(&named),
        [(home.join(".aws/config"), HomeState::NotCovered(UndoReason::Masked))],
        "a masked file runs with no copy"
    );
    let mut found = states(&walked);
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        found,
        [
            (home.join("work/.ssh"), HomeState::NotCovered(UndoReason::Masked)),
            (home.join("work/notes.md"), HomeState::File { mode: MODE }),
        ],
        "the walk never goes into a mask"
    );
    assert!(!walked.covers(&CoverAsk::HomeDir(home.join("work"))));
    assert!(
        floor.unwrap_err().contains("secret or a masked file"),
        "a masked floor stops the call"
    );
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("work")), &masks, LIMITS), Coverage::Masked);
    assert_eq!(
        coverage(&CoverAsk::Home(home.join(".aws/config")), &masks, LIMITS),
        Coverage::Masked
    );
}

#[test]
fn home_copy_dir_over_limit_says_not_covered() {
    let (_temp, root) = root();
    let home = root.join("home");
    for n in 0..3 {
        write(&home.join(format!("big/{n}.txt")), "x\n");
    }
    let small = HomeLimits { max_files: 2, max_bytes: 1024 };
    let bytes = HomeLimits { max_files: 10, max_bytes: 4 };

    let prepared = prepare(&[dir(&home.join("big"))], &[], small).unwrap();

    assert_eq!(
        states(&prepared),
        [(home.join("big"), HomeState::NotCovered(UndoReason::OverLimit))]
    );
    assert_eq!(prepared.not_covered, [(home.join("big"), UndoReason::OverLimit)]);
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("big")), &[], small), Coverage::OverLimit);
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("big")), &[], bytes), Coverage::OverLimit);
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("big")), &[], LIMITS), Coverage::Covered);
}

#[tokio::test]
async fn home_copy_of_a_parent_bind_walks_the_dir() {
    let (_temp, root) = root();
    let home = root.join("home");
    let conf = home.join(".config/app/app.conf");
    write(&conf, "a=1\n");
    write(&home.join(".config/app/theme.conf"), "dark\n");
    let parent = home.join(".config/app");
    let targets = [file(&conf), dir(&parent)];
    let store = store(&root);
    let (conversation, turn) = ids();

    let before = prepare(&targets, &[], LIMITS).unwrap();
    let copy = store.home_before(conversation, turn, Reader::new(&before)).await.unwrap();
    write(&conf, "a=2\n");
    write(&parent.join("new.conf"), "made by the call\n");
    let after_copy = after(&before, &[], LIMITS);
    let deltas = store.home_after(&copy, Reader::new(&after_copy)).await.unwrap();
    let backups = changed(&deltas, &before, &after_copy);

    assert_eq!(before.walked, std::slice::from_ref(&parent));
    assert_eq!(before.entries.len(), 2, "the target and the other file of its dir");
    let paths: Vec<(&Path, bool)> =
        backups.iter().map(|backup| (backup.path.as_path(), backup.reason.is_none())).collect();
    assert_eq!(paths, [(conf.as_path(), true), (parent.join("new.conf").as_path(), true)]);
}

#[test]
fn home_copy_of_a_large_parent_stops_at_the_limit() {
    let (_temp, root) = root();
    let home = root.join("home");
    let conf = home.join("app/app.conf");
    write(&conf, "a=1\n");
    for n in 0..5 {
        write(&home.join(format!("app/data/{n}")), "x\n");
    }
    let small = HomeLimits { max_files: 3, max_bytes: 1024 * 1024 };

    let prepared = prepare(&[file(&conf), dir(&home.join("app"))], &[], small).unwrap();

    assert_eq!(
        states(&prepared),
        [
            (conf.clone(), HomeState::File { mode: MODE }),
            (home.join("app"), HomeState::NotCovered(UndoReason::OverLimit)),
        ],
        "the target alone, and the dir is not covered"
    );
    assert!(prepared.walked.is_empty());
    assert!(prepared.covers(&CoverAsk::Home(conf)));
    assert!(!prepared.covers(&CoverAsk::HomeDir(home.join("app"))));
}

#[test]
fn home_copy_failure_of_a_floor_stops_the_call() {
    let (_temp, root) = root();
    let home = root.join("home");
    let big = home.join(".zshrc");
    std::fs::create_dir_all(&home).unwrap();
    let file = std::fs::File::create(&big).unwrap();
    file.set_len(efr_snapshot::MAX_HOME_FILE_BYTES + 1).unwrap();
    let notes = home.join("notes.bin");
    std::fs::copy(&big, &notes).unwrap();

    let floor = prepare(&[floor(&big)], &[], LIMITS);
    let other = prepare(&[self::file(&notes)], &[], LIMITS).unwrap();

    let reason = floor.unwrap_err();
    assert!(reason.contains("larger than 16 MiB") && reason.contains("did not run"), "{reason}");
    assert_eq!(states(&other), [(notes, HomeState::NotCovered(UndoReason::OverLimit))]);
}

#[test]
fn old_undo_trash_is_collected() {
    let (_temp, root) = root();
    let trash = root.join("undo-trash");
    let clock = TestClock::new();
    let rng = efr_test_support::TestRng::new(3);
    let day = Duration::from_secs(24 * 3600);
    let old = format!("u-{}", efr_stdx::id::uuid_v7(&clock, &rng).hyphenated());
    clock.advance(10 * day);
    let new = format!("u-{}", efr_stdx::id::uuid_v7(&clock, &rng).hyphenated());
    for name in [&old, &new, &"kept".to_owned()] {
        write(&trash.join(name).join("home/x"), "x\n");
    }
    let now = SystemTime::from(efr_stdx::time::Clock::now(&clock));

    let removed = collect_trash(&trash, 5 * day, now);

    assert_eq!(removed, 1);
    assert!(!trash.join(&old).exists() && trash.join(&new).exists());
    assert!(trash.join("kept").exists(), "a name that is not an undo stays");
}

#[tokio::test]
async fn home_copy_names_an_uncovered_file_of_a_walked_dir_only_when_it_changed() {
    let (_temp, root) = root();
    let home = root.join("home");
    let docs = home.join("Documents");
    write(&docs.join("notes.md"), "notes\n");
    let big = docs.join("video.mkv");
    std::fs::File::create(&big).unwrap().set_len(20 * 1024 * 1024).unwrap();
    let targets = [file(&docs.join("notes.md")), dir(&docs)];
    let store = store(&root);
    let (conversation, turn) = ids();

    let before = prepare(&targets, &[], LIMITS).unwrap();
    let copy = store.home_before(conversation, turn, Reader::new(&before)).await.unwrap();
    write(&docs.join("notes.md"), "changed\n");
    let after_copy = after(&before, &[], LIMITS);
    let deltas = store.home_after(&copy, Reader::new(&after_copy)).await.unwrap();
    let backups = changed(&deltas, &before, &after_copy);

    let paths: Vec<&Path> = backups.iter().map(|backup| backup.path.as_path()).collect();
    assert_eq!(paths, [docs.join("notes.md").as_path()], "the untouched large file stays quiet");

    // The call grows the large file: no copy covers it, and the turn says so.
    let copy = store.home_before(conversation, turn, Reader::new(&after_copy)).await.unwrap();
    std::fs::OpenOptions::new().append(true).open(&big).unwrap().set_len(21 * 1024 * 1024).unwrap();
    let grown = after(&after_copy, &[], LIMITS);
    let deltas = store.home_after(&copy, Reader::new(&grown)).await.unwrap();
    let backups = changed(&deltas, &after_copy, &grown);
    let reasons: Vec<(&Path, Option<UndoReason>)> =
        backups.iter().map(|backup| (backup.path.as_path(), backup.reason)).collect();
    assert_eq!(reasons, [(big.as_path(), Some(UndoReason::OverLimit))]);
}

#[test]
fn a_path_inside_a_snapshot_root_needs_no_home_copy() {
    let roots = [PathBuf::from("/home/u/p/app"), PathBuf::from("/home/u/.local/share/efr/scratch")];
    let home = |path: &str| CoverAsk::Home(PathBuf::from(path));

    assert!(inside(&home("/home/u/p/app/.env"), &roots));
    assert!(inside(&CoverAsk::HomeDir(PathBuf::from("/home/u/p/app/src")), &roots));
    assert!(!inside(&home("/home/u/p/app"), &roots), "the root itself is no path inside it");
    assert!(!inside(&home("/home/u/p/app-two/.env"), &roots));
    assert!(!inside(&home("/home/u/.zshrc"), &roots));
    assert!(!inside(&CoverAsk::CronTable, &roots));
}
