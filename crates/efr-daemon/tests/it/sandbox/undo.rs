//! Undo of the `shell` lines of a turn in `auto` (efr's auto spec 10.5 to 10.8, 16.4):
//! a destructive line that the turn's snapshot covers runs at once, with no review and
//! no question, and `turn.undo` brings back its files and refs; a contained line that
//! rewrites the project's git history and reaches for the snapshot store and the undo
//! trash cannot break the undo; a write grant outside the roots gets its home copy
//! before the call. The tests drive a real zsh: the first two with the real `efr-sbx`
//! under `just test-sandbox`, else with the fake launcher, and the home copy with the
//! fake launcher, which keeps the spec of each call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    Event, EventEnvelope, Grant, Method, Mode, UndoAction,
};
use efr_test_daemon::TestDaemon;
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    REVIEWER_ALLOWS, ScriptedModel, completed, fake_launcher, git, kinds, ready, run_turn, skip,
    specs, with_zsh,
};
use crate::home::home_copies;
use crate::support::zsh_enabled;
use crate::undo::{entry, files_below, read, undo};

/// The launcher that runs the calls of a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launcher {
    /// The fake launcher, which runs each line with no sandbox.
    Fake,
    /// The real `efr-sbx` of `EFR_TEST_SBX_BIN`, which `just test-sandbox` sets.
    Real,
}

/// Dirs below the target dir, not /tmp, which the real sandbox replaces with its own.
fn dirs() -> Arc<TestDirs> {
    Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap())
}

/// The real launcher of `EFR_TEST_SBX_BIN`, which `just test-sandbox` sets.
fn real_bin() -> Option<PathBuf> {
    efr_stdx::env::path(efr_stdx::env::Var::TestSbxBin).ok().flatten()
}

/// A daemon in `auto` over `model` with the real launcher when `EFR_TEST_SBX_BIN` names
/// one, else with the fake launcher; `None` when the real sandbox is not ready here.
async fn start(
    test: &str,
    dirs: &Arc<TestDirs>,
    model: Arc<ScriptedModel>,
) -> Option<(TestDaemon, Launcher)> {
    let real = real_bin();
    let builder = with_zsh(TestDaemon::builder()).dirs(Arc::clone(dirs)).custom_provider(model);
    let Some(launcher) = real else {
        let (launcher, _) = fake_launcher(dirs);
        let daemon =
            builder.sandbox_launcher(&launcher).probe_override(ready()).start().await.unwrap();
        return Some((daemon, Launcher::Fake));
    };
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = std::collections::BTreeMap::from([
        ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.to_string_lossy().into_owned()),
    ]);
    let daemon = builder.shell_env(env).sandbox_launcher(&launcher).start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        drop(client);
        daemon.stop().await.unwrap();
        return None;
    }
    Some((daemon, Launcher::Real))
}

/// Registers `path` as a project.
async fn register(daemon: &TestDaemon, path: &Path) {
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: path.to_path_buf(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
}

/// git's standard output in `dir`, trimmed, without the machine's configuration;
/// `None` when git fails.
async fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let out = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .await
        .unwrap();
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Commits every file of `dir` with the message `message`; the new commit's id.
async fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]).await;
    git(dir, &["commit", "-q", "-m", message]).await;
    git_out(dir, &["rev-parse", "HEAD"]).await.unwrap()
}

/// Asserts that no exit of the turn went to the reviewer or to the user.
fn asks_nobody(events: &[EventEnvelope], model: &ScriptedModel) {
    let kinds = kinds(events);
    for kind in ["exit_requested", "exit_review_started", "approval_requested"] {
        assert!(!kinds.contains(&kind), "{kind}: {kinds:?}");
    }
    assert_eq!(model.reviews.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn shell_covered_destructive_lines_run_and_undo_brings_them_back() {
    let test = "shell_covered_destructive_lines_run_and_undo_brings_them_back";
    if !zsh_enabled(test) {
        return;
    }
    let dirs = dirs();
    let project = dirs.create_dir("home/project").unwrap();
    git(&project, &["init", "-q", "-b", "main"]).await;
    let a = project.join("a.txt");
    std::fs::write(&a, "v1\n").unwrap();
    let first = commit(&project, "one").await;
    std::fs::write(&a, "v2\n").unwrap();
    commit(&project, "two").await;
    std::fs::write(&a, "v3\n").unwrap();
    std::fs::write(project.join("b.txt"), "b\n").unwrap();
    let third = commit(&project, "three").await;
    let log = project.join("important.log");
    std::fs::write(&log, "a line of the log\n").unwrap();
    let app = dirs.create_dir("home/p/app").unwrap();
    let db = app.join("db.sqlite");
    std::fs::write(&db, "SQLite format 3\0 the user's data\n").unwrap();
    // NOTE: corpus rows 54, 57 and 59. The dd of row 57 has a count, so it ends, and a
    // path that zsh expands: zsh leaves the `~` of `of=~/...` as it is.
    let lines = [
        "git reset --hard HEAD~2",
        "dd if=/dev/zero of=../p/app/db.sqlite bs=16 count=1 status=none",
        ": > important.log",
    ];
    let calls = lines.iter().map(|line| ("shell", json!({ "command": line }))).collect();
    let model = ScriptedModel::reviewed(calls, REVIEWER_ALLOWS);
    let Some((daemon, launcher)) = start(test, &dirs, Arc::clone(&model)).await else { return };
    register(&daemon, &project).await;
    register(&daemon, &app).await;

    let (_, events) = run_turn(
        &daemon,
        "drop the last two commits, zero the app database and empty important.log",
        Mode::Auto,
        ApprovalDecision::Deny,
    )
    .await;

    asks_nobody(&events, &model);
    let [(reset, _), _, _] = completed(&events).try_into().unwrap();
    assert!(reset.contains("HEAD is now at"), "{launcher:?}: {reset}");
    assert_eq!(git_out(&project, &["rev-parse", "HEAD"]).await.unwrap(), first);
    assert_eq!(read(&a), "v1\n");
    assert!(!project.join("b.txt").exists());
    assert_eq!(std::fs::read(&db).unwrap(), [0; 16]);
    assert_eq!(read(&log), "");
    let info = events.iter().find_map(|envelope| match &envelope.event {
        Event::TurnCompleted { undo, .. } => undo.clone(),
        _ => None,
    });
    assert!(info.is_some_and(|info| info.undoable), "the turn's end says undo can take it back");

    let undone = undo(&daemon, 10, |_| {}).await.unwrap();

    assert!(undone.applied, "{undone:?}");
    assert_eq!(
        undone.entries,
        [
            entry(UndoAction::Restore, "a.txt"),
            entry(UndoAction::Restore, "b.txt"),
            entry(UndoAction::Restore, "important.log"),
            entry(UndoAction::Restore, "~/p/app/db.sqlite"),
        ]
    );
    let [moved] = undone.refs.try_into().unwrap();
    assert_eq!((moved.root.as_str(), moved.name.as_str()), ("project", "refs/heads/main"));
    assert_eq!((moved.applied, moved.reason), (Some(true), None));
    assert!(first.starts_with(moved.from.as_deref().unwrap()), "{moved:?}");
    assert!(third.starts_with(moved.to.as_deref().unwrap()), "{moved:?}");
    assert_eq!(git_out(&project, &["rev-parse", "main"]).await.unwrap(), third, "the branch");
    assert_eq!(read(&a), "v3\n");
    assert_eq!(read(&project.join("b.txt")), "b\n");
    assert_eq!(std::fs::read(&db).unwrap(), b"SQLite format 3\0 the user's data\n");
    assert_eq!(read(&log), "a line of the log\n");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_snapshot_unreachable_from_sandbox() {
    let test = "shell_snapshot_unreachable_from_sandbox";
    if !zsh_enabled(test) {
        return;
    }
    let dirs = dirs();
    let project = dirs.create_dir("home/project").unwrap();
    git(&project, &["init", "-q", "-b", "main"]).await;
    let a = project.join("a.txt");
    std::fs::write(&a, "v1\n").unwrap();
    let main = commit(&project, "one").await;
    git(&project, &["checkout", "-q", "-b", "feature"]).await;
    std::fs::write(project.join("f.txt"), "feature work\n").unwrap();
    let feature = commit(&project, "feature").await;
    git(&project, &["checkout", "-q", "main"]).await;
    // NOTE: the trash of an earlier undo, which a contained line must not read or empty.
    let data = dirs.dirs().data().to_path_buf();
    let earlier = data.join("undo-trash/u-earlier");
    std::fs::create_dir_all(&earlier).unwrap();
    std::fs::write(earlier.join("keep.txt"), "kept\n").unwrap();
    let store = data.join("snapshots");
    let trash = data.join("undo-trash");
    // NOTE: the line hides in `sh -c`, so efr predicts no exit and the line runs
    // contained. It rewrites the project's history so that the commit of `feature` is
    // gone from the project, and plants a `.git` file that names the store. Only the real
    // sandbox masks the store and the undo trash, so only then does the line reach for
    // them: it lists, plants, writes alternates into each store and deletes.
    let rewrite = "git update-ref -d refs/heads/feature; echo deleted=$?; \
                   git reflog expire --expire=now --all; echo expired=$?; \
                   git gc --quiet --prune=now; echo pruned=$?; \
                   mkdir -p sub && echo \"gitdir: $S\" > sub/.git; echo planted=$?";
    let reach = "; ls -A \"$S\" \"$T\"; echo listed=$?; \
                 touch \"$S/planted\" \"$T/planted\"; echo touched=$?; \
                 for d in \"$S\"/*.git; do echo /x > \"$d/objects/info/alternates\"; done; \
                 rm -rf \"$S\"/* \"$T\"/*; echo removed=$?";
    let real = real_bin().is_some();
    let script = if real { format!("{rewrite}{reach}") } else { rewrite.to_owned() };
    let line = format!(
        "echo changed > a.txt; S={} T={} sh -c '{script}'",
        store.display(),
        trash.display()
    );
    let model =
        ScriptedModel::reviewed(vec![("shell", json!({ "command": line }))], REVIEWER_ALLOWS);
    let Some((daemon, launcher)) = start(test, &dirs, Arc::clone(&model)).await else { return };
    register(&daemon, &project).await;

    let (_, events) =
        run_turn(&daemon, "tidy the repository", Mode::Auto, ApprovalDecision::Deny).await;

    asks_nobody(&events, &model);
    let [(output, _)] = completed(&events).try_into().unwrap();
    for done in ["deleted=0", "expired=0", "pruned=0", "planted=0"] {
        assert!(output.contains(done), "{launcher:?}: {done}: {output}");
    }
    assert_eq!(git_out(&project, &["rev-parse", "--verify", "--quiet", "feature"]).await, None);
    let object = format!("{feature}^{{commit}}");
    assert_eq!(git_out(&project, &["cat-file", "-e", &object]).await, None, "pruned");
    if launcher == Launcher::Real {
        assert!(output.contains("touched=1"), "{output}");
        assert!(!output.contains("keep.txt") && !output.contains(".root"), "{output}");
        assert!(!store.join("planted").exists() && !trash.join("planted").exists());
        assert_eq!(read(&earlier.join("keep.txt")), "kept\n", "the undo trash stays");
        for entry in std::fs::read_dir(&store).unwrap().flatten() {
            let alternates = entry.path().join("objects/info/alternates");
            assert!(!alternates.exists(), "{alternates:?}");
        }
    }

    let undone = undo(&daemon, 10, |_| {}).await.unwrap();

    assert!(undone.applied, "{undone:?}");
    // NOTE: git takes no path with a `.git` part, so no snapshot holds the planted
    // `sub/.git`, and undo leaves it as it is (plan PB3).
    assert_eq!(undone.entries, [entry(UndoAction::Restore, "a.txt")]);
    assert!(project.join("sub/.git").is_file());
    let [moved] = undone.refs.try_into().unwrap();
    assert_eq!((moved.root.as_str(), moved.name.as_str()), ("project", "refs/heads/feature"));
    assert_eq!((moved.from, moved.applied, moved.reason), (None, Some(true), None));
    assert!(feature.starts_with(moved.to.as_deref().unwrap()), "{feature}");
    assert_eq!(read(&a), "v1\n");
    assert_eq!(
        git_out(&project, &["rev-parse", "feature"]).await.as_deref(),
        Some(feature.as_str()),
        "the branch and its commit come back from the mirror"
    );
    assert!(git_out(&project, &["cat-file", "-e", &object]).await.is_some());
    assert_eq!(git_out(&project, &["rev-parse", "main"]).await.unwrap(), main);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_home_copy_before_write_grant() {
    if !zsh_enabled("shell_home_copy_before_write_grant") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let documents = dirs.create_dir("home/Documents").unwrap();
    let notes = documents.join("notes.txt");
    std::fs::write(&notes, "old notes\n").unwrap();
    let line = json!({ "command": "echo 'new notes' > ~/Documents/notes.txt" });
    let model = ScriptedModel::reviewed(vec![("shell", line)], REVIEWER_ALLOWS);
    let (launcher, fake) = fake_launcher(&dirs);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(Arc::clone(&model) as Arc<dyn efr_provider::Provider>)
        .start()
        .await
        .unwrap();

    let (_, events) = run_turn(
        &daemon,
        "write new notes into ~/Documents/notes.txt",
        Mode::Auto,
        ApprovalDecision::Deny,
    )
    .await;

    let kinds = kinds(&events);
    let at = |kind: &str| kinds.iter().position(|known| *known == kind).unwrap();
    assert!(at("exit_judged") < at("tool_call_completed"), "{kinds:?}");
    // NOTE: the grant of an existing file opens its directory (the bind `Parent`), so
    // efrd copies what the directory holds before the call.
    let [spec] = specs(&fake).try_into().unwrap();
    assert_eq!(spec.launch, efr_sandbox::SpecLaunch::Contained);
    assert_eq!(spec.grants, [Grant::Write { path: documents.clone() }]);
    assert_eq!(read(&notes), "new notes\n");
    assert_eq!(home_copies(&dirs).await, [(notes.clone(), "old notes\n".to_owned())]);
    let changed: Vec<(PathBuf, bool)> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::HomeFileChanged { path, covered, .. } => Some((path.clone(), *covered)),
            _ => None,
        })
        .collect();
    assert_eq!(changed, [(notes.clone(), true)]);

    let undone = undo(&daemon, 10, |_| {}).await.unwrap();

    assert!(undone.applied);
    assert_eq!(undone.entries, [entry(UndoAction::Restore, "~/Documents/notes.txt")]);
    assert_eq!(read(&notes), "old notes\n");
    let below = notes.strip_prefix("/").unwrap();
    assert_eq!(
        files_below(&undone.trash.unwrap()),
        [(Path::new("home").join(below), "new notes\n".to_owned())]
    );
    daemon.stop().await.unwrap();
}
