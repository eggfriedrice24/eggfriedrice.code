use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use efr_config::{FileState, Settings};
use efr_protocol::ProjectId;
use efr_test_support::{TestClock, TestDirs, Wait};
use pretty_assertions::assert_eq;
use rustix::fs::inotify::ReadFlags;
use tokio::sync::watch;

use super::inotify::{Event, Inotify};
use super::{DEBOUNCE, Watched, names};
use crate::testing::{deps, serve_with};

/// The kernel's limit of queued events of one inotify instance, or `None` with a
/// message when it is too high to fill with files in a test.
#[expect(clippy::print_stderr, reason = "a skipped test says why, as atuin's e2e tests do")]
pub(crate) fn max_queued_events(test: &str) -> Option<usize> {
    let text = std::fs::read_to_string("/proc/sys/fs/inotify/max_queued_events").unwrap();
    let max: usize = text.trim().parse().unwrap();
    if max > 1 << 17 {
        eprintln!("skipping {test}: max_queued_events is {max}, too many files to create");
        return None;
    }
    Some(max)
}

/// Creates `count` empty files in `dir`; each is a create and a close after a write.
pub(crate) fn flood(dir: &Path, count: usize) {
    for n in 0..count {
        std::fs::File::create(dir.join(format!("flood-{n}"))).unwrap();
    }
}

fn event(wd: i32, flags: ReadFlags, name: &str) -> Event {
    Event { wd, flags, name: Some(name.into()) }
}

/// A link `config.toml` in `/c/efr` to `efr.toml` in the dotfiles, watched as 1 and 2.
fn linked() -> (tempfile::TempDir, Watched) {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("efr.toml");
    std::fs::write(&target, "").unwrap();
    let path = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let dirs = BTreeMap::from([("/c/efr".into(), 1), ("/home/u/dotfiles".into(), 2)]);
    let watched = Watched { dirs, names: names(&FileState::of(&path)), stale: false };
    (dir, watched)
}

#[test]
fn only_events_that_name_the_file_its_target_or_the_registry_concern_it() {
    let (_dir, mut watched) = linked();

    assert!(watched.concerns(&event(1, ReadFlags::MOVED_TO, "config.toml")));
    assert!(watched.concerns(&event(2, ReadFlags::CLOSE_WRITE, "efr.toml")));
    assert!(watched.concerns(&event(2, ReadFlags::CREATE, "efr.toml")));
    assert!(watched.concerns(&event(2, ReadFlags::DELETE, "efr.toml")));
    assert!(watched.concerns(&event(1, ReadFlags::MOVED_FROM, "config.toml")));
    assert!(watched.concerns(&event(1, ReadFlags::MOVED_TO, "projects.toml")), "the registry");
    assert!(!watched.concerns(&event(1, ReadFlags::CREATE, "notes.toml")));
    assert!(!watched.concerns(&event(1, ReadFlags::CREATE, ".config.toml.swp")));
    assert!(!watched.concerns(&event(7, ReadFlags::CREATE, "config.toml")), "a removed watch");
    assert!(!watched.stale);
}

#[test]
fn a_lost_track_concerns_the_file_and_makes_the_watches_stale() {
    let lost = [
        Event { wd: -1, flags: ReadFlags::QUEUE_OVERFLOW, name: None },
        Event { wd: 2, flags: ReadFlags::IGNORED, name: None },
        Event { wd: 1, flags: ReadFlags::DELETE_SELF, name: None },
        Event { wd: 1, flags: ReadFlags::MOVE_SELF, name: None },
        Event { wd: 2, flags: ReadFlags::UNMOUNT, name: None },
    ];
    for event in lost {
        let (_dir, mut watched) = linked();
        assert!(watched.concerns(&event), "{event:?}");
        assert!(watched.stale, "{event:?}");
    }

    let (_dir, mut watched) = linked();
    let removed = Event { wd: 7, flags: ReadFlags::IGNORED, name: None };
    assert!(!watched.concerns(&removed), "the end of a watch that follow removed");
    assert!(!watched.stale);
}

#[test]
fn every_event_of_a_batch_is_looked_at() {
    let (_dir, mut watched) = linked();
    let batch = [
        event(1, ReadFlags::MOVED_TO, "config.toml"),
        Event { wd: -1, flags: ReadFlags::QUEUE_OVERFLOW, name: None },
    ];

    assert!(watched.concern(&batch));
    assert!(watched.stale, "the overflow after a concerning event still counts");
}

#[tokio::test]
async fn a_missing_directory_is_watched_through_the_nearest_one_that_exists() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("c/efr/config.toml");
    let inotify = Inotify::new().unwrap();
    let mut watched = Watched::default();

    watched.follow(&inotify, &FileState::of(&path));
    assert_eq!(watched.dirs.keys().collect::<Vec<_>>(), [root.path()]);
    assert!(watched.names.contains(std::ffi::OsStr::new("c")));

    std::fs::create_dir(root.path().join("c")).unwrap();
    assert!(watched.concern(&inotify.next().await.unwrap()));
    watched.follow(&inotify, &FileState::of(&path));
    assert_eq!(watched.dirs.keys().collect::<Vec<_>>(), [&root.path().join("c")]);
    assert!(watched.names.contains(std::ffi::OsStr::new("efr")));

    std::fs::create_dir(root.path().join("c/efr")).unwrap();
    assert!(watched.concern(&inotify.next().await.unwrap()));
    watched.follow(&inotify, &FileState::of(&path));
    assert_eq!(watched.dirs.keys().collect::<Vec<_>>(), [&root.path().join("c/efr")]);
    assert_eq!(watched.names, names(&FileState::of(&path)));
}

#[tokio::test]
async fn a_retargeted_link_moves_the_watch_and_the_old_one_ends_quietly() {
    let root = tempfile::tempdir().unwrap();
    for dir in ["c", "a", "b"] {
        std::fs::create_dir(root.path().join(dir)).unwrap();
    }
    let path = root.path().join("c/config.toml");
    std::os::unix::fs::symlink(root.path().join("a/config.toml"), &path).unwrap();
    let inotify = Inotify::new().unwrap();
    let mut watched = Watched::default();
    watched.follow(&inotify, &FileState::of(&path));
    let old = watched.dirs[&root.path().join("a")];

    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(root.path().join("b/config.toml"), &path).unwrap();
    assert!(watched.concern(&inotify.next().await.unwrap()));
    watched.follow(&inotify, &FileState::of(&path));

    let dirs: Vec<_> = watched.dirs.keys().cloned().collect();
    assert_eq!(dirs, [root.path().join("b"), root.path().join("c")]);
    let ended = inotify.next().await.unwrap();
    assert_eq!(ended, [Event { wd: old, flags: ReadFlags::IGNORED, name: None }]);
    assert!(!watched.concern(&ended));
    assert!(!watched.stale);
}

#[tokio::test]
async fn stale_watches_are_armed_again() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    let inotify = Inotify::new().unwrap();
    let mut watched = Watched::default();
    watched.follow(&inotify, &FileState::of(&path));
    let before = watched.dirs.clone();

    watched.stale = true;
    watched.follow(&inotify, &FileState::of(&path));

    assert!(!watched.stale);
    assert_eq!(watched.dirs.keys().collect::<Vec<_>>(), before.keys().collect::<Vec<_>>());
    assert_ne!(watched.dirs, before, "a new watch");
    let ended = inotify.next().await.unwrap();
    assert!(!watched.concern(&ended), "the end of the old watch: {ended:?}");
}

#[tokio::test]
async fn reading_the_file_never_reloads_it() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let path = dirs.dirs().config().join("config.toml");
    std::fs::write(&path, "[shell]\nidle_minutes = 9\n").unwrap();
    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_config_watch()).await;

    for _ in 0..20 {
        std::fs::read_to_string(&path).unwrap();
        for _ in 0..1000 {
            tokio::task::yield_now().await;
        }
    }

    assert!(!clock.requested_sleeps().contains(&DEBOUNCE), "{:?}", clock.requested_sleeps());
    daemon.stop().await;
}

/// Moves the clock past each quiet time the watcher starts until the settings change.
async fn until_reloaded(clock: &TestClock, settings: &mut watch::Receiver<Arc<Settings>>) {
    let mut seen = 0;
    Wait::new("a reload by the watcher")
        .until(|| {
            if settings.has_changed().unwrap() {
                settings.borrow_and_update();
                return true;
            }
            let quiet = quiet_times(clock);
            if quiet > seen {
                seen = quiet;
                clock.advance(DEBOUNCE);
            }
            false
        })
        .await
        .unwrap();
}

/// Waits, without moving the clock, until the settings pass `test`.
async fn until(settings: &mut watch::Receiver<Arc<Settings>>, test: impl Fn(&Settings) -> bool) {
    Wait::new("settings that pass the test")
        .until(|| test(&settings.borrow_and_update()))
        .await
        .unwrap();
}

/// How many quiet times the watcher has started.
fn quiet_times(clock: &TestClock) -> usize {
    clock.requested_sleeps().iter().filter(|sleep| **sleep == DEBOUNCE).count()
}

/// Lets the watcher read and handle what is queued.
async fn settle() {
    for _ in 0..20_000 {
        tokio::task::yield_now().await;
    }
}

/// Writes `text` the way vim does: a new file in the same directory, renamed over the
/// old one.
fn save_by_rename(path: &Path, text: &str) {
    let temp = path.with_file_name(".4913");
    std::fs::write(&temp, text).unwrap();
    std::fs::rename(&temp, path).unwrap();
}

#[tokio::test]
async fn a_save_before_the_watches_were_armed_reloads_once_they_are() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let path = dirs.dirs().config().join("config.toml");
    std::fs::create_dir_all(dirs.dirs().config()).unwrap();
    // The daemon starts with the settings it loaded before this save.
    std::fs::write(&path, "[shell]\nidle_minutes = 9\n").unwrap();

    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_config_watch()).await;
    let mut settings = daemon.settings.subscribe();
    until(&mut settings, |settings| settings.shell.idle_minutes == 9).await;

    assert_eq!(quiet_times(&clock), 0, "no event started a quiet time");
    daemon.stop().await;
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

    std::fs::write(&path, "[shell]\nidle_minutes = 12\n").unwrap();
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 12, "a write in place");

    // vim with a backup: the old file moves away, the new one is written, and the
    // backup is removed.
    let backup = path.with_file_name("config.toml~");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::write(&path, "[shell]\nidle_minutes = 13\n").unwrap();
    std::fs::remove_file(&backup).unwrap();
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 13, "a save that moves the old file away");
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

#[tokio::test]
async fn a_project_registered_while_the_daemon_runs_reaches_the_engine() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_config_watch()).await;
    let engine = daemon.engine.subscribe();
    let id: ProjectId = "0192f0c1-7a00-7000-8000-0000000000aa".parse().unwrap();
    let root = dirs.home().join("p/app");
    std::fs::create_dir_all(&root).unwrap();

    let registry = format!("[[project]]\nid = \"{id}\"\nroot = \"{}\"\n", root.display());
    save_by_rename(&dirs.dirs().config().join("projects.toml"), &registry);
    // NOTE: the reload at the watcher's start may read the registry too; only an event
    // of the registry starts a quiet time.
    Wait::new("a quiet time started by the registry's change")
        .until(|| quiet_times(&clock) > 0)
        .await
        .unwrap();
    clock.advance(DEBOUNCE);
    Wait::new("the project in the engine")
        .until(|| engine.borrow().locations().project_root(&id).is_some())
        .await
        .unwrap();

    assert_eq!(engine.borrow().locations().project_root(&id), Some(root.as_path()));
    daemon.stop().await;
}

/// Points the link `link` at `target` the way `ln -sfn` does: a new link renamed over
/// the old one.
fn retarget(link: &Path, target: &Path) {
    let temp = link.with_file_name(".config.toml.link");
    std::os::unix::fs::symlink(target, &temp).unwrap();
    std::fs::rename(&temp, link).unwrap();
}

#[tokio::test]
async fn a_retargeted_link_reloads_and_follows_the_new_target() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let old = dirs.root().join("dotfiles/a/config.toml");
    let new = dirs.root().join("dotfiles/b/config.toml");
    for (target, text) in
        [(&old, "[conversation]\nmax_queued = 4\n"), (&new, "[conversation]\nmax_queued = 5\n")]
    {
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, text).unwrap();
    }
    let link = dirs.dirs().config().join("config.toml");
    std::os::unix::fs::symlink(&old, &link).unwrap();
    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_config_watch()).await;
    let mut settings = daemon.settings.subscribe();
    // The watcher reads the file once it watches it.
    until(&mut settings, |settings| settings.conversation.max_queued == 4).await;

    retarget(&link, &new);
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().conversation.max_queued, 5);

    let quiet = quiet_times(&clock);
    save_by_rename(&old, "[conversation]\nmax_queued = 6\n");
    settle().await;
    assert_eq!(quiet_times(&clock), quiet, "the old target is no longer watched");

    save_by_rename(&new, "[conversation]\nmax_queued = 7\n");
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().conversation.max_queued, 7, "the new target is watched");
    daemon.stop().await;
}

#[tokio::test]
async fn a_removed_and_recreated_target_directory_reloads() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let dotfiles = dirs.root().join("dotfiles/efr");
    std::fs::create_dir_all(&dotfiles).unwrap();
    let target = dotfiles.join("config.toml");
    std::fs::write(&target, "[conversation]\nmax_queued = 4\n").unwrap();
    std::os::unix::fs::symlink(&target, dirs.dirs().config().join("config.toml")).unwrap();
    let mut running = Settings::default();
    running.conversation.max_queued = 4;
    let daemon = serve_with(running, deps(&dirs, &clock).with_config_watch()).await;
    let mut settings = daemon.settings.subscribe();

    std::fs::remove_dir_all(&dotfiles).unwrap();
    until_reloaded(&clock, &mut settings).await;
    let defaults = Settings::default().conversation.max_queued;
    assert_eq!(settings.borrow().conversation.max_queued, defaults, "a dangling link is no file");

    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(&target, "[conversation]\nmax_queued = 8\n").unwrap();
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().conversation.max_queued, 8);

    save_by_rename(&target, "[conversation]\nmax_queued = 9\n");
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().conversation.max_queued, 9, "the new directory is watched");
    daemon.stop().await;
}

#[tokio::test]
async fn a_lost_queue_arms_the_watches_again_and_reloads_once() {
    let Some(max) = max_queued_events("a_lost_queue_arms_the_watches_again_and_reloads_once")
    else {
        return;
    };
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let config = dirs.dirs().config();
    std::fs::create_dir_all(config).unwrap();
    std::fs::write(config.join("config.toml"), "[shell]\nidle_minutes = 7\n").unwrap();
    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_config_watch()).await;
    let mut settings = daemon.settings.subscribe();
    // The watcher reads the file once it watches it; the flood comes after that.
    until(&mut settings, |settings| settings.shell.idle_minutes == 7).await;

    // NOTE: the watcher runs on this thread, so it reads nothing until the test
    // awaits: the flood fills the queue, and the event of the config file is lost.
    flood(config, max / 2 + 64);
    std::fs::write(config.join("config.toml"), "[shell]\nidle_minutes = 9\n").unwrap();
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 9);
    settle().await;
    assert_eq!(quiet_times(&clock), 1, "one reload for the overflow");

    save_by_rename(&config.join("config.toml"), "[shell]\nidle_minutes = 10\n");
    until_reloaded(&clock, &mut settings).await;
    assert_eq!(settings.borrow().shell.idle_minutes, 10, "the watches work again");
    daemon.stop().await;
}
