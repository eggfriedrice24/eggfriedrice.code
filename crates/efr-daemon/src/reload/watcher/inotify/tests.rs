use std::ffi::OsString;

use pretty_assertions::assert_eq;
use rustix::fs::inotify::ReadFlags;

use super::{Event, Inotify};
use crate::reload::watcher::tests::{flood, max_queued_events};

/// Reads events until one matches `want`, and returns every event read up to it.
async fn until(inotify: &Inotify, want: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let events = inotify.next().await.unwrap();
        let done = events.iter().any(&want);
        seen.extend(events);
        if done {
            return seen;
        }
    }
}

fn named(flags: ReadFlags, name: &str) -> impl Fn(&Event) -> bool {
    let name = OsString::from(name);
    move |event| event.flags.contains(flags) && event.name.as_ref() == Some(&name)
}

#[tokio::test]
async fn the_descriptor_is_non_blocking_and_closed_on_exec() {
    let inotify = Inotify::new().unwrap();
    let fd = inotify.fd.get_ref();

    assert!(rustix::io::fcntl_getfd(fd).unwrap().contains(rustix::io::FdFlags::CLOEXEC));
    assert!(rustix::fs::fcntl_getfl(fd).unwrap().contains(rustix::fs::OFlags::NONBLOCK));
}

#[tokio::test]
async fn a_save_by_rename_is_seen_and_a_read_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let inotify = Inotify::new().unwrap();
    let wd = inotify.add(dir.path()).unwrap();

    std::fs::read_to_string(&path).unwrap();
    let temp = dir.path().join(".4913");
    std::fs::write(&temp, "[shell]\n").unwrap();
    std::fs::rename(&temp, &path).unwrap();
    let events = until(&inotify, named(ReadFlags::MOVED_TO, "config.toml")).await;

    let seen: Vec<(ReadFlags, Option<OsString>)> =
        events.iter().map(|event| (event.flags, event.name.clone())).collect();
    assert_eq!(
        seen,
        [
            (ReadFlags::CREATE, Some(".4913".into())),
            (ReadFlags::CLOSE_WRITE, Some(".4913".into())),
            (ReadFlags::MOVED_FROM, Some(".4913".into())),
            (ReadFlags::MOVED_TO, Some("config.toml".into())),
        ],
        "the read of the file before the save left no event"
    );
    assert!(events.iter().all(|event| event.wd == wd));
}

#[tokio::test]
async fn a_write_in_place_and_a_removal_are_seen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let inotify = Inotify::new().unwrap();
    inotify.add(dir.path()).unwrap();

    std::fs::write(&path, "[shell]\n").unwrap();
    until(&inotify, named(ReadFlags::CLOSE_WRITE, "config.toml")).await;
    std::fs::remove_file(&path).unwrap();
    until(&inotify, named(ReadFlags::DELETE, "config.toml")).await;
}

#[tokio::test]
async fn a_removed_directory_ends_its_watch() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("efr");
    std::fs::create_dir(&dir).unwrap();
    let inotify = Inotify::new().unwrap();
    let wd = inotify.add(&dir).unwrap();

    std::fs::remove_dir(&dir).unwrap();
    let events = until(&inotify, |event| event.flags.contains(ReadFlags::IGNORED)).await;

    assert!(events.iter().any(|event| event.flags.contains(ReadFlags::DELETE_SELF)));
    assert!(events.iter().all(|event| event.wd == wd));
}

#[tokio::test]
async fn a_removed_watch_reports_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let inotify = Inotify::new().unwrap();
    let wd = inotify.add(dir.path()).unwrap();

    inotify.remove(wd).unwrap();

    let events = inotify.next().await.unwrap();
    assert_eq!(events, [Event { wd, flags: ReadFlags::IGNORED, name: None }]);
}

#[tokio::test]
async fn a_file_is_no_directory_to_watch() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.toml");
    std::fs::write(&file, "").unwrap();
    let inotify = Inotify::new().unwrap();

    let error = inotify.add(&file).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::NotADirectory);
    assert_eq!(
        inotify.add(&dir.path().join("missing")).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[tokio::test]
async fn a_full_queue_reports_an_overflow() {
    let Some(max) = max_queued_events("a_full_queue_reports_an_overflow") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let inotify = Inotify::new().unwrap();
    inotify.add(dir.path()).unwrap();

    flood(dir.path(), max / 2 + 64);
    let events = until(&inotify, |event| event.flags.contains(ReadFlags::QUEUE_OVERFLOW)).await;

    let overflow = events.last().unwrap();
    assert_eq!(overflow.wd, -1);
    assert_eq!(overflow.name, None);
}
