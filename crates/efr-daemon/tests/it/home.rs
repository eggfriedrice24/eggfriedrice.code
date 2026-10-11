//! The home store of efrd (efr's auto spec 10.8, phase B), end to end: an approved
//! write outside the snapshot roots copies the file into
//! `$XDG_DATA_HOME/efr/snapshots/home.git` before the call, and the turn records the
//! change. [`home_copies`] reads the store, for these tests and those of `sandbox`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{ApprovalDecision, Event, Mode};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, kinds, reviewed_daemon, run_turn};

/// Runs git on the home store of `dirs` with git's own config switched off; its
/// standard output, `None` when git fails.
async fn git(dirs: &TestDirs, args: &[&str]) -> Option<Vec<u8>> {
    let store = dirs.dirs().data().join("snapshots/home.git");
    let out = efr_stdx::process::command("git", dirs.dirs().data())
        .arg("--git-dir")
        .arg(&store)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .await
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Each file that a copy of the home store of `dirs` holds from before its call, with
/// its text, sorted: the state that `efr undo` gives it back. A link is left out.
pub(crate) async fn home_copies(dirs: &TestDirs) -> Vec<(PathBuf, String)> {
    let Some(refs) = git(dirs, &["for-each-ref", "--format=%(refname)", "refs/efr/"]).await else {
        return Vec::new();
    };
    let mut copies = Vec::new();
    for name in String::from_utf8_lossy(&refs).lines().filter(|name| name.contains("/home/")) {
        // NOTE: the ref names the "after" commit, a child of the "before" one, once the
        // call ended.
        let before = format!("{name}^");
        let before = if git(dirs, &["rev-parse", "--verify", "--quiet", &before]).await.is_some() {
            before
        } else {
            name.to_owned()
        };
        let Some(listed) = git(dirs, &["ls-tree", "-r", &before, "files/"]).await else {
            continue;
        };
        // NOTE: a link on the way to a file is a copy of its own, with mode 120000.
        let files = String::from_utf8_lossy(&listed)
            .lines()
            .filter(|line| !line.starts_with("120000 "))
            .filter_map(|line| line.split_once('\t').map(|(_, file)| file.to_owned()))
            .collect::<Vec<_>>();
        for file in &files {
            let file = file.as_str();
            let spec = format!("{before}:{file}");
            let text = git(dirs, &["cat-file", "blob", &spec]).await.unwrap_or_default();
            let path = Path::new("/").join(file.strip_prefix("files/").unwrap_or(file));
            copies.push((path, String::from_utf8_lossy(&text).into_owned()));
        }
    }
    copies.sort();
    copies
}

#[tokio::test]
async fn home_copy_of_a_reviewed_write_outside_the_roots() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("Documents/notes.md");
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(&notes, "old notes\n").unwrap();
    let write = json!({ "path": notes, "content": "new notes\n" });
    let model = ScriptedModel::reviewed(vec![("write_file", write)], REVIEWER_ALLOWS);
    let daemon = reviewed_daemon(&dirs, Arc::clone(&model)).await;

    let (_, events) = run_turn(
        &daemon,
        "put new notes in ~/Documents/notes.md",
        Mode::Auto,
        ApprovalDecision::Allow,
    )
    .await;

    let kinds = kinds(&events);
    assert!(kinds.contains(&"tool_call_completed"), "{kinds:?}");
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "new notes\n");
    assert_eq!(home_copies(&dirs).await, [(notes.clone(), "old notes\n".to_owned())]);
    let changed: Vec<(PathBuf, bool)> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::HomeFileChanged { path, floor, .. } => Some((path.clone(), *floor)),
            _ => None,
        })
        .collect();
    assert_eq!(changed, [(notes, false)]);
    assert!(!dirs.dirs().data().join("file-backups").exists(), "the phase A store stays empty");
    daemon.stop().await.unwrap();
}
