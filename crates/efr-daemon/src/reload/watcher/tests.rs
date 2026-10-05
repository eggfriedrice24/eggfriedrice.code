use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::{FileState, Settings};
use efr_test_support::{TestClock, TestDirs};
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::{DEBOUNCE, Watched, names};
use crate::testing::{deps, serve_with};

fn event(path: &str) -> notify::Result<notify::Event> {
    Ok(notify::Event::new(notify::EventKind::Any).add_path(PathBuf::from(path)))
}

fn watched(file: &FileState) -> Watched {
    Watched { dirs: Default::default(), names: names(file) }
}

#[test]
fn only_events_that_name_the_file_or_its_target_concern_it() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("efr.toml");
    std::fs::write(&target, "").unwrap();
    let path = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let watched = watched(&FileState::of(&path));

    assert!(watched.concerns(&event("/c/efr/config.toml")));
    assert!(watched.concerns(&event("/home/u/dotfiles/efr.toml")));
    assert!(!watched.concerns(&event("/c/efr/projects.toml")));
    assert!(!watched.concerns(&event("/c/efr/.config.toml.swp")));
    let rescan = notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan);
    assert!(watched.concerns(&Ok(rescan)));
    assert!(watched.concerns(&Err(notify::Error::generic("lost"))));
}

/// Moves the clock past each quiet time the watcher starts until the settings change.
async fn until_reloaded(clock: &TestClock, settings: &mut watch::Receiver<Arc<Settings>>) {
    let mut seen = 0;
    for _ in 0..1_000_000 {
        if settings.has_changed().unwrap() {
            settings.borrow_and_update();
            return;
        }
        let quiet = clock.requested_sleeps().iter().filter(|sleep| **sleep == DEBOUNCE).count();
        if quiet > seen {
            seen = quiet;
            clock.advance(DEBOUNCE);
        }
        tokio::task::yield_now().await;
    }
    panic!("the watcher never reloaded");
}

/// Writes `text` the way vim does: a new file in the same directory, renamed over the
/// old one.
fn save_by_rename(path: &Path, text: &str) {
    let temp = path.with_file_name(".4913");
    std::fs::write(&temp, text).unwrap();
    std::fs::rename(&temp, path).unwrap();
}

#[tokio::test]
async fn a_saved_file_reloads_after_the_quiet_time() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_config_watch()).await;
    let mut settings = daemon.settings.subscribe();
    let path = dirs.dirs().config().join("config.toml");

    std::fs::write(&path, "[shell]\nidle_minutes = 9\n").unwrap();
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 9);

    save_by_rename(&path, "[shell]\nidle_minutes = 10\n");
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 10);

    std::fs::remove_file(&path).unwrap();
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 60, "no file is the defaults");

    std::fs::write(&path, "[shell]\nidle_minutes = 11\n").unwrap();
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 11, "a file created again");
    daemon.stop().await;
}

#[tokio::test]
async fn an_edit_of_the_target_of_a_symlinked_file_reloads() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let dotfiles = dirs.root().join("dotfiles/efr");
    std::fs::create_dir_all(&dotfiles).unwrap();
    let target = dotfiles.join("config.toml");
    std::fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, dirs.dirs().config().join("config.toml")).unwrap();
    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_config_watch()).await;
    let mut settings = daemon.settings.subscribe();

    save_by_rename(&target, "[conversation]\nmax_queued = 4\n");
    until_reloaded(&clock, &mut settings).await;

    assert_eq!(settings.borrow().conversation.max_queued, 4);
    assert!(
        std::fs::symlink_metadata(dirs.dirs().config().join("config.toml")).unwrap().is_symlink()
    );
    daemon.stop().await;
}
