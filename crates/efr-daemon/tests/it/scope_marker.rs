//! The scope marker's projects file and the repository hint (DV52, DV53), over the
//! [`Place`] of `routing.rs`: `app` (`~/p/app`) and `lib` (`~/p/lib`) are registered,
//! and `~/p/tool` is a directory outside every project.
//!
//! efrd writes `<runtime root>/projects.tsv` for the zsh plugin, which reads it with no
//! fork. A prompt outside projects in a git work tree gets the hint at most once a day
//! for each work tree; the tests move the `TestClock` only after the turn before it
//! ended (`Place::at` waits for that), and the hint rule has no sleep.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminProjectRemove, AdminProjectRemoveResult, Method,
};
use efr_test_daemon::{FakeProcView, TestDaemon, TestDaemonBuilder};
use efr_test_support::TestDirs;
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
