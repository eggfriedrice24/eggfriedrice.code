//! The scope marker's projects file and the repository hint (DV52, DV53), over the
//! [`Place`] of `routing.rs`: `app` (`~/p/app`) and `lib` (`~/p/lib`) are registered,
//! and `~/p/tool` is a directory outside every project.
//!
//! efrd writes `<runtime root>/projects.tsv` for the zsh plugin, which reads it with no
//! fork. A prompt outside projects in a git work tree gets the hint at most once a day
//! for each work tree; the tests move the `TestClock` only after the turn before it
//! ended (`Place::at` waits for that), and the hint rule has no sleep.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminProjectRemove, AdminProjectRemoveResult, Method,
    PromptSendResult, RepoHint,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{FakeProcView, TestDaemon, TestDaemonBuilder};
use efr_test_support::{TestClock, TestDirs};
use pretty_assertions::assert_eq;

use crate::routing::{Place, tree};

/// The text of the projects file of `place`'s daemon.
fn projects_file(place: &Place) -> String {
    let file = place.daemon.dirs().dirs().runtime().join("projects.tsv");
    std::fs::read_to_string(file).unwrap()
}

/// The lines that the registry of [`tree`] gives: the deepest root first, then by
/// root; `lib` has no name of its own, so it is the last part of its root.
fn registered(place: &Place) -> String {
    format!("{}\tapp\n{}\tlib\n", place.app.display(), place.lib.display())
}

/// Makes `dir` a git work tree, with the system and global configuration off.
async fn git_init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let status = efr_stdx::process::command("git", dir)
        .args(["init", "-q"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .await
        .unwrap();
    assert!(status.success());
}

fn hint(sent: &PromptSendResult) -> Option<&RepoHint> {
    sent.route.as_ref().and_then(|route| route.hint.as_ref())
}

/// The hint of `root`, as efrd names it: with its links resolved, as git does.
fn hint_of(root: &Path) -> RepoHint {
    RepoHint { root: std::fs::canonicalize(root).unwrap() }
}

#[tokio::test]
async fn projects_file_at_start_and_after_add_and_remove() {
    let place = Place::start().await;
    assert_eq!(projects_file(&place), registered(&place));
    let client = place.daemon.client().await.unwrap();
    let tool = place.home.join("p/tool");

    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: tool.clone(),
        name: Some("tool".to_owned()),
        git_root: false,
    });
    let added: AdminProjectAddResult = client.call(add).await.unwrap();
    let root = added.project.root;
    assert_eq!(
        projects_file(&place),
        format!(
            "{}\tapp\n{}\tlib\n{}\ttool\n",
            place.app.display(),
            place.lib.display(),
            root.display()
        )
    );

    let remove = Method::AdminProjectRemove(AdminProjectRemove { path: place.lib.clone() });
    let _: AdminProjectRemoveResult = client.call(remove).await.unwrap();
    assert_eq!(
        projects_file(&place),
        format!("{}\tapp\n{}\ttool\n", place.app.display(), root.display())
    );
    place.stop().await;
}

#[tokio::test]
async fn projects_file_follows_a_hand_edit_at_the_next_prompt() {
    let place = Place::start().await;
    let registry = place.daemon.dirs().dirs().config().join("projects.toml");
    let text = std::fs::read_to_string(&registry).unwrap();
    let tool = place.home.join("p/tool");
    std::fs::write(
        &registry,
        format!(
            "{text}\n[[project]]\nid = \"0192f0c1-7a00-7000-8000-0000000a4403\"\nroot = \"{}\"\n",
            tool.display()
        ),
    )
    .unwrap();
    assert_eq!(projects_file(&place), registered(&place), "no prompt read it yet");

    place.at(1, &place.home, "pts-1", 100).await;

    assert_eq!(projects_file(&place), format!("{}{}\ttool\n", registered(&place), tool.display()));
    place.stop().await;
}

#[tokio::test]
async fn projects_file_leaves_out_a_root_with_a_tab() {
    let (dirs, procs) = tree();
    let odd = dirs.home().join("p/a\tb");
    std::fs::create_dir_all(&odd).unwrap();
    let registry = dirs.dirs().config().join("projects.toml");
    let text = std::fs::read_to_string(&registry).unwrap();
    std::fs::write(
        &registry,
        format!(
            "{text}\n[[project]]\nid = \"0192f0c1-7a00-7000-8000-0000000a4403\"\nroot = \"{}\"\n",
            odd.display().to_string().replace('\t', "\\t")
        ),
    )
    .unwrap();
    let daemon = start(&dirs, &procs, |builder| builder).await;
    let place = Place::on(daemon, procs, &dirs);

    assert_eq!(projects_file(&place), registered(&place));
    place.stop().await;
}

#[tokio::test]
async fn projects_file_is_private() {
    let place = Place::start().await;
    let file = place.daemon.dirs().dirs().runtime().join("projects.tsv");
    let mode = std::fs::metadata(file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    place.stop().await;
}

/// A daemon of [`Place`] on the tree `dirs`.
async fn start(
    dirs: &Arc<TestDirs>,
    procs: &FakeProcView,
    change: impl FnOnce(TestDaemonBuilder) -> TestDaemonBuilder,
) -> TestDaemon {
    let builder = TestDaemon::builder()
        .dirs(Arc::clone(dirs))
        .proc_view(procs.clone())
        .config(|config| config.jobs.max_wakes_per_hour = 0);
    change(builder).start().await.unwrap()
}

#[tokio::test]
async fn repo_hint_once_per_repo_per_day() {
    let place = Place::start().await;
    let tool = place.home.join("p/tool");
    git_init(&tool).await;
    std::fs::create_dir_all(tool.join("src")).unwrap();

    let first = place.at(1, &tool, "pts-1", 100).await;
    let second = place.at(2, &tool.join("src"), "pts-1", 100).await;
    // NOTE: `at` waited for the second turn's end; the hint rule reads the clock and has
    // no sleep. The slot also ends for idleness now, which this test does not check.
    place.daemon.clock().advance(Duration::from_secs(24 * 3600));
    let third = place.at(3, &tool, "pts-1", 100).await;

    assert_eq!(hint(&first), Some(&hint_of(&tool)));
    assert_eq!(hint(&second), None, "a second prompt in the same day has none");
    assert_eq!(hint(&third), Some(&hint_of(&tool)));
    place.stop().await;
}

#[tokio::test]
async fn repo_hint_survives_a_restart() {
    let mut place = Place::start_with(TestDaemonBuilder::persistent).await;
    let tool = place.home.join("p/tool");
    git_init(&tool).await;
    let first = place.at(1, &tool, "pts-1", 100).await;

    place.daemon.restart().await.unwrap();
    let second = place.at(2, &tool, "pts-1", 100).await;

    assert_eq!(hint(&first), Some(&hint_of(&tool)));
    assert_eq!(hint(&second), None);
    let file = place.daemon.dirs().dirs().state().join("repo-hints.json");
    let mode = std::fs::metadata(file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    place.stop().await;
}

#[tokio::test]
async fn repo_hint_off_by_setting() {
    let place =
        Place::start_with(|builder| builder.config(|config| config.conversation.repo_hint = false))
            .await;
    let tool = place.home.join("p/tool");
    git_init(&tool).await;

    let sent = place.at(1, &tool, "pts-1", 100).await;

    assert_eq!(hint(&sent), None);
    assert!(!place.daemon.dirs().dirs().state().join("repo-hints.json").exists());
    place.stop().await;
}

#[tokio::test]
async fn no_repo_hint_in_a_project_or_at_home() {
    let place = Place::start().await;
    git_init(&place.app).await;
    // A home that is a git repository (dotfiles) is never one for this rule.
    git_init(&place.home).await;

    let app = place.at(1, &place.app, "pts-1", 100).await;
    let home = place.at(2, &place.home, "pts-1", 100).await;
    let notes = place.at(3, &place.notes, "pts-1", 100).await;

    assert_eq!([hint(&app), hint(&home), hint(&notes)], [None, None, None]);
    place.stop().await;
}

#[tokio::test]
async fn repo_hint_file_drops_old_entries() {
    let (dirs, procs) = tree();
    let clock = TestClock::new();
    let now = clock.now();
    let state = dirs.dirs().state().to_path_buf();
    std::fs::create_dir_all(&state).unwrap();
    let old = now - jiff::SignedDuration::from_hours(30);
    let recent = now - jiff::SignedDuration::from_hours(2);
    std::fs::write(
        state.join("repo-hints.json"),
        format!(r#"{{"/old/repo": "{old}", "/recent/repo": "{recent}"}}"#),
    )
    .unwrap();
    let daemon = start(&dirs, &procs, |builder| builder.clock(clock)).await;
    let place = Place::on(daemon, procs, &dirs);
    let tool = place.home.join("p/tool");
    git_init(&tool).await;

    let sent = place.at(1, &tool, "pts-1", 100).await;

    assert_eq!(hint(&sent), Some(&hint_of(&tool)));
    let held: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join("repo-hints.json")).unwrap()).unwrap();
    let mut roots: Vec<PathBuf> = held.as_object().unwrap().keys().map(PathBuf::from).collect();
    roots.sort();
    let mut expected = vec![PathBuf::from("/recent/repo"), hint_of(&tool).root];
    expected.sort();
    assert_eq!(roots, expected);
    place.stop().await;
}
