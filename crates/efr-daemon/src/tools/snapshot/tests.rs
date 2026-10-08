use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::Settings;
use efr_conversation::CallContext;
use efr_permissions::{Engine, Locations};
use efr_protocol::{CallId, ChangeKind, ConversationId, TurnId};
use efr_scope::{Git, Home};
use efr_snapshot::{DEFAULT_SNAPSHOT_TIMEOUT, SnapshotParts, Snapshots};
use efr_test_support::TestClock;
use efr_tools::{WrittenDiff, WrittenFile, WrittenKind, written_diff};
use pretty_assertions::assert_eq;

use super::{CallSnapshots, with_shown_header};

fn snapshots(home: &Path, settings: Settings) -> CallSnapshots {
    let clock = TestClock::new();
    let store = Snapshots::new(SnapshotParts {
        dir: home.join("data/snapshots"),
        git: Git::new(clock.shared()).isolated(),
        home: Home::new(home).unwrap(),
        clock: clock.shared(),
        excludes_file: None,
        timeout: DEFAULT_SNAPSHOT_TIMEOUT,
    });
    let engine = Engine::with_defaults(Locations::new(home).unwrap());
    let (_, engine) = tokio::sync::watch::channel(Arc::new(engine));
    let (_, settings) = tokio::sync::watch::channel(Arc::new(settings));
    CallSnapshots::new(store, engine, settings, Home::new(home).unwrap())
}

fn call(home: &Path) -> CallContext {
    CallContext::new(
        ConversationId::from_uuid(uuid::Uuid::from_u128(1)),
        TurnId::from_uuid(uuid::Uuid::from_u128(2)),
        CallId::from_uuid(uuid::Uuid::from_u128(3)),
        home,
        home.join("scratch"),
    )
}

#[test]
fn the_header_of_a_written_diff_names_the_shown_path() {
    let path = Path::new("/home/u/p/a.rs");
    let diff = written_diff(path, Some("x\n"), "y\n", 2000).unwrap();
    assert_eq!(
        with_shown_header(&diff.text, (path, "a.rs"), (path, "a.rs")),
        "--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,1 @@\n-x\n+y\n"
    );
    let new = written_diff(path, None, "y\n", 2000).unwrap();
    assert!(
        with_shown_header(&new.text, (path, "~/p/a.rs"), (path, "~/p/a.rs"))
            .starts_with("--- /dev/null\n+++ b/~/p/a.rs\n")
    );
}

#[test]
fn a_write_shows_as_a_change_with_its_diff_under_scratch_home_or_absolute() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let snapshots = snapshots(home, Settings::default());
    let call = call(home);
    let file = |path: PathBuf, old: Option<&str>| {
        vec![WrittenFile {
            diff: written_diff(&path, old, "one\ntwo\n", 2000),
            kind: if old.is_none() { WrittenKind::Created } else { WrittenKind::Changed },
            binary: false,
            path,
        }]
    };

    let (changes, diff) = snapshots.written(&call, &file(home.join("scratch/notes.md"), None));
    let changes = changes.unwrap();
    assert_eq!(changes.files[0].path, "$SCRATCH/notes.md");
    assert_eq!(changes.files[0].kind, ChangeKind::Added);
    assert_eq!((changes.added, changes.removed), (2, 0));
    assert!(diff.unwrap().starts_with("--- /dev/null\n+++ b/$SCRATCH/notes.md\n"));

    let (changes, _) = snapshots.written(&call, &file(home.join("p/x.rs"), Some("one\n")));
    let changes = changes.unwrap();
    assert_eq!(changes.files[0].path, "~/p/x.rs");
    assert_eq!(changes.files[0].kind, ChangeKind::Modified);

    let (changes, diff) =
        snapshots.written(&call, &file(PathBuf::from("/etc/x.conf"), Some("a\n")));
    assert_eq!(changes.unwrap().files[0].path, "/etc/x.conf");
    assert!(diff.unwrap().starts_with("--- a/etc/x.conf\n+++ b/etc/x.conf\n"));

    let same = file(home.join("p/same.rs"), Some("one\ntwo\n"));
    assert_eq!(snapshots.written(&call, &same), (None, None), "no change, no line");

    let binary = [WrittenFile {
        path: home.join("p/blob"),
        kind: WrittenKind::Changed,
        binary: true,
        diff: None,
    }];
    let (changes, diff) = snapshots.written(&call, &binary);
    assert!(changes.unwrap().files[0].binary);
    assert_eq!(diff, None);
}

#[test]
fn a_patch_shows_every_file_with_its_kind_and_one_diff_after_another() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let snapshots = snapshots(home, Settings::default());
    let call = call(home);
    let diff =
        |text: String, added: usize, removed: usize| Some(WrittenDiff { text, added, removed });
    let (a, gone, from, to) =
        (home.join("p/a.rs"), home.join("p/gone.rs"), home.join("p/old.rs"), home.join("p/new.rs"));
    let written = [
        WrittenFile {
            path: a.clone(),
            kind: WrittenKind::Changed,
            binary: false,
            diff: diff(format!("--- a{0}\n+++ b{0}\n@@ -1,1 +1,1 @@\n-x\n+y\n", a.display()), 1, 1),
        },
        WrittenFile {
            path: gone.clone(),
            kind: WrittenKind::Deleted,
            binary: false,
            diff: diff(
                format!("--- a{}\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-z\n", gone.display()),
                0,
                1,
            ),
        },
        WrittenFile {
            path: to.clone(),
            kind: WrittenKind::Moved { from: from.clone() },
            binary: false,
            diff: diff(format!("--- a{}\n+++ b{}\n", from.display(), to.display()), 0, 0),
        },
        WrittenFile {
            path: home.join("p/same.rs"),
            kind: WrittenKind::Changed,
            binary: false,
            diff: None,
        },
    ];

    let (changes, text) = snapshots.written(&call, &written);

    let changes = changes.unwrap();
    let listed: Vec<(&str, ChangeKind, Option<&str>)> = changes
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.kind, file.from.as_deref()))
        .collect();
    assert_eq!(
        listed,
        [
            ("~/p/a.rs", ChangeKind::Modified, None),
            ("~/p/gone.rs", ChangeKind::Deleted, None),
            ("~/p/new.rs", ChangeKind::Renamed, Some("~/p/old.rs")),
        ],
        "sorted by path, the unchanged file left out"
    );
    assert_eq!((changes.added, changes.removed), (1, 2));
    assert_eq!(
        text.unwrap(),
        "--- a/~/p/a.rs\n+++ b/~/p/a.rs\n@@ -1,1 +1,1 @@\n-x\n+y\n\
         --- a/~/p/gone.rs\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-z\n\
         --- a/~/p/old.rs\n+++ b/~/p/new.rs\n"
    );
}

#[tokio::test]
async fn snapshots_off_take_none() {
    let home = tempfile::tempdir().unwrap();
    let mut settings = Settings::default();
    settings.snapshot.enabled = false;
    let snapshots = snapshots(home.path(), settings);
    let call = call(home.path());
    std::fs::create_dir_all(home.path().join("scratch")).unwrap();
    assert!(snapshots.before_call(&call, &[]).await.is_none());
    assert!(!home.path().join("data/snapshots").exists());
}

#[tokio::test]
async fn a_shell_call_lists_what_it_changed_in_scratch() {
    let home = tempfile::tempdir().unwrap();
    let snapshots = snapshots(home.path(), Settings::default());
    let call = call(home.path());
    std::fs::create_dir_all(home.path().join("scratch")).unwrap();
    let before = snapshots.before_call(&call, &[]).await;
    std::fs::write(home.path().join("scratch/out.txt"), "1\n2\n").unwrap();
    let changes = snapshots.after_call(before).await.unwrap();
    assert_eq!(changes.files[0].path, "$SCRATCH/out.txt");
    let turn = snapshots.finish_turn(call.conversation_id, call.turn_id).await.unwrap();
    assert_eq!(turn, changes);
}
