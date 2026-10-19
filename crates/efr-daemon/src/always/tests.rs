//! The writer of always rules: the load check first, the form and the comments of the
//! file, a duplicate, a file that changes under it, a read-only file, a link, and the
//! reload wait.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::{CONFIG_FILE, Settings};
use efr_conversation::AllowWriter as _;
use efr_permissions::{AllowRule, Engine, Locations};
use efr_protocol::{AdminConfigReloadResult, AlwaysSaved, NotSavedReason};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::{RuleWriter, TRIES, write_rule, write_rule_with};
use crate::reload::{Reloads, Request};

/// A config file with comments and one table that the writer must keep.
const COMMENTED: &str = "\
# my settings
[model]
name = \"test-model\" # the model I use

[sandbox]
# the dirs that every call may write
write_roots = []
";

fn write(under: &str) -> AllowRule {
    AllowRule::Write { under: under.to_owned(), project: None }
}

/// A home directory with efr's config dir in it, and the path of `config.toml`.
fn home() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join(".config/efr");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(dir.path().join("notes")).unwrap();
    let file = config.join(CONFIG_FILE);
    (dir, file)
}

/// The always rules of the file at `path`.
fn rules(path: &Path) -> Vec<AllowRule> {
    let text = std::fs::read_to_string(path).ok();
    Settings::parse(path, text.as_deref()).unwrap().sandbox.allow
}

/// A writer for the home `home`, and the requests of its reloads.
fn writer(home: &Path) -> (RuleWriter, tokio::sync::mpsc::Receiver<Request>, TestClock) {
    let engine = Engine::with_defaults(Locations::new(home).unwrap());
    let (_sender, engine) = watch::channel(Arc::new(engine));
    let (reloads, requests) = Reloads::new();
    let clock = TestClock::new();
    let config = home.join(".config/efr");
    (RuleWriter::new(&config, engine, reloads, clock.shared()), requests, clock)
}

/// Answers every reload request of `requests` as applied.
fn applied(mut requests: tokio::sync::mpsc::Receiver<Request>) {
    tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            request.answer(AdminConfigReloadResult {
                applied: true,
                error: None,
                restart_needed: Vec::new(),
            });
        }
    });
}

#[test]
fn a_rule_joins_the_file_and_every_comment_stays() {
    let (_dir, path) = home();
    std::fs::write(&path, COMMENTED).unwrap();
    assert_eq!(write_rule(&path, &write("~/notes")), Ok(0));
    assert_eq!(write_rule(&path, &write("~/docs")), Ok(1));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(COMMENTED.trim_end()), "{text}");
    assert!(text.contains("[[sandbox.allow]]"), "{text}");
    assert_eq!(rules(&path), vec![write("~/notes"), write("~/docs")]);
}

#[test]
fn a_missing_file_gets_the_rule() {
    let (_dir, path) = home();
    assert_eq!(write_rule(&path, &write("~/notes")), Ok(0));
    assert_eq!(rules(&path), vec![write("~/notes")]);
}

#[test]
fn always_duplicate_rule_returns_its_index() {
    let (_dir, path) = home();
    std::fs::write(&path, COMMENTED).unwrap();
    write_rule(&path, &write("~/notes")).unwrap();
    write_rule(&path, &write("~/docs")).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_eq!(write_rule(&path, &write("~/docs")), Ok(1));
    assert_eq!(std::fs::read(&path).unwrap(), before, "the file stays as it was");
}

#[test]
fn always_retries_on_a_changed_file() {
    let (_dir, path) = home();
    std::fs::write(&path, COMMENTED).unwrap();
    // Another writer adds a rule between the first plan and its write: the second plan
    // sees it, and the rule lands after it.
    let mut changed = false;
    let other = path.clone();
    let mut change_once = || {
        if !changed {
            changed = true;
            write_rule(&other, &write("~/docs")).unwrap();
        }
    };
    assert_eq!(write_rule_with(&path, &write("~/notes"), &mut change_once), Ok(1));
    assert_eq!(rules(&path), vec![write("~/docs"), write("~/notes")]);

    // A file that changes before every write: the writer gives up after its tries.
    let mut count = 0;
    let other = path.clone();
    let mut change_always = || {
        count += 1;
        let text = std::fs::read_to_string(&other).unwrap();
        std::fs::write(&other, format!("{text}# change {count}\n")).unwrap();
    };
    assert_eq!(
        write_rule_with(&path, &write("~/more"), &mut change_always),
        Err(NotSavedReason::Changed)
    );
    assert_eq!(count, TRIES);
    assert_eq!(rules(&path), vec![write("~/docs"), write("~/notes")]);
}

#[test]
fn a_read_only_file_is_not_written() {
    let (_dir, path) = home();
    std::fs::write(&path, COMMENTED).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert_eq!(write_rule(&path, &write("~/notes")), Err(NotSavedReason::ReadOnly));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
}

#[test]
fn a_linked_file_is_written_behind_its_link() {
    let (dir, path) = home();
    let real = dir.path().join("dotfiles/efr.toml");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, COMMENTED).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    assert_eq!(write_rule(&path, &write("~/notes")), Ok(0));
    assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink(), "the link stays");
    assert_eq!(rules(&real), vec![write("~/notes")]);
    // A link to nothing is refused: the file would appear where nobody expects it.
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(dir.path().join("gone.toml"), &path).unwrap();
    assert_eq!(write_rule(&path, &write("~/docs")), Err(NotSavedReason::Io));
    assert!(!dir.path().join("gone.toml").exists());
}

#[tokio::test]
async fn always_writer_runs_the_load_check_before_the_write() {
    let (dir, path) = home();
    std::fs::write(&path, COMMENTED).unwrap();
    let (writer, requests, _clock) = writer(dir.path());
    let outcome = writer.add(write("~/.config/systemd/user")).await;
    assert_eq!(outcome, AlwaysSaved::NotSaved { reason: NotSavedReason::LoadCheck });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED, "byte for byte");
    drop(requests);
}

#[tokio::test]
async fn a_saved_rule_waits_for_the_reload() {
    let (dir, path) = home();
    std::fs::write(&path, COMMENTED).unwrap();
    let (writer, requests, _clock) = writer(dir.path());
    applied(requests);
    let outcome = writer.add(write("~/notes")).await;
    assert_eq!(outcome, AlwaysSaved::Saved { index: 0, rule: "write under ~/notes".to_owned() });
    assert_eq!(writer.add(write("~/notes")).await, outcome, "a duplicate keeps its index");
    assert_eq!(rules(&path), vec![write("~/notes")]);
}

#[tokio::test]
async fn a_reload_that_does_not_run_is_reported() {
    let (dir, path) = home();
    std::fs::write(&path, COMMENTED).unwrap();
    let (writer, requests, _clock) = writer(dir.path());
    drop(requests);
    let outcome = writer.add(write("~/notes")).await;
    assert_eq!(outcome, AlwaysSaved::NotSaved { reason: NotSavedReason::ReloadTimeout });
    assert_eq!(rules(&path), vec![write("~/notes")], "the rule is in the file");
}
