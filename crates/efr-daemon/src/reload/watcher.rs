//! The config file watcher: reloads when `config.toml` changes.
//!
//! It watches the config root and, when `config.toml` is a symbolic link, the directory
//! of the file the link resolves to, so an edit in a dotfiles repository reloads too.
//! Directories are watched, not the file: an editor that saves by writing a new file
//! and renaming it over the old one (vim, nvim) replaces the file the watch would hold.
//! A burst of events (a save is often several) becomes one reload after
//! [`DEBOUNCE`] of quiet, timed by the injected clock; a read of the file is no event.
//! Before each reload the link is resolved again and the watched directories follow it,
//! so a link pointed elsewhere is followed too. A removed file reloads as no file, as
//! at start, and a file created again reloads as itself. When the kernel lost events
//! (its queue overflowed) or a watched directory went away, the watches are armed again
//! and the file reloads once.
//!
//! The watcher is its own, on inotify ([`inotify`]): efrd runs on Linux only.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_config::{CONFIG_FILE, FileState};
use rustix::fs::inotify::ReadFlags;
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::reload::{file_state, reload};
use crate::state::State;

use self::inotify::{Event, Inotify};

mod inotify;

/// The quiet after the last event before the reload starts.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(200);

/// The events that say the watcher lost track of a directory: it was removed, moved or
/// unmounted, or its watch ended.
const LOST: ReadFlags = ReadFlags::IGNORED
    .union(ReadFlags::DELETE_SELF)
    .union(ReadFlags::MOVE_SELF)
    .union(ReadFlags::UNMOUNT);

/// The directories under watch and the names in them that can change the config file.
#[derive(Debug, Default)]
pub(crate) struct Watched {
    /// Each watched directory and its watch. Two paths of one directory share a watch.
    dirs: BTreeMap<PathBuf, i32>,
    /// The names of the file and its link target, and the first missing part of each
    /// wanted directory that does not exist.
    names: BTreeSet<OsString>,
    /// True when the watches must be armed again from nothing: events were lost, or a
    /// watched directory went away.
    stale: bool,
}

impl Watched {
    /// True when `event` may change the config file: it names one of the names in a
    /// directory under watch, or the watcher lost track and must take a fresh look. A
    /// lost track also marks the watches stale, so the next `follow` arms them again.
    fn concerns(&mut self, event: &Event) -> bool {
        if event.flags.contains(ReadFlags::QUEUE_OVERFLOW) {
            self.stale = true;
            return true;
        }
        // NOTE: a watch that `follow` removed still reports what was queued before,
        // and then IN_IGNORED; its directory no longer matters.
        if !self.dirs.values().any(|wd| *wd == event.wd) {
            return false;
        }
        if event.flags.intersects(LOST) {
            self.stale = true;
            return true;
        }
        event.name.as_ref().is_some_and(|name| self.names.contains(name))
    }

    /// True when any of `events` concerns the file. Each event is looked at, so a lost
    /// track anywhere in the batch marks the watches stale.
    fn concern(&mut self, events: &[Event]) -> bool {
        let mut any = false;
        for event in events {
            any |= self.concerns(event);
        }
        any
    }

    /// Watches the directories of `file` and stops watching the ones it no longer has.
    /// Stale watches are dropped first and armed again.
    fn follow(&mut self, inotify: &Inotify, file: &FileState) {
        if std::mem::take(&mut self.stale) {
            let stale: BTreeSet<i32> = std::mem::take(&mut self.dirs).into_values().collect();
            for wd in stale {
                // A watch whose directory was removed is gone already.
                let _ = inotify.remove(wd);
            }
        }
        let mut names = names(file);
        let mut dirs = BTreeMap::new();
        for dir in file.watched_dirs() {
            // NOTE: inotify_add_watch is one path lookup, cheap enough for the async
            // worker; the file state, which may read a chain of links, was taken on
            // the blocking pool.
            if let Some((watched, wd)) = self.arm(inotify, &dir, &mut names) {
                dirs.insert(watched, wd);
            }
        }
        for (dir, wd) in &self.dirs {
            if !dirs.contains_key(dir) && !dirs.values().any(|kept| kept == wd) {
                // A directory that was removed is no longer watched anyway.
                let _ = inotify.remove(*wd);
            }
        }
        self.dirs = dirs;
        self.names = names;
    }

    /// Watches `dir`, or, while it is missing, the nearest directory above it that
    /// exists, with the name of the next part added to `names`: when that part appears,
    /// the reload that follows watches one level deeper, down to `dir` itself.
    fn arm(
        &self,
        inotify: &Inotify,
        dir: &Path,
        names: &mut BTreeSet<OsString>,
    ) -> Option<(PathBuf, i32)> {
        let mut at = dir;
        loop {
            if let Some(wd) = self.dirs.get(at) {
                return Some((at.to_path_buf(), *wd));
            }
            match inotify.add(at) {
                Ok(wd) => return Some((at.to_path_buf(), wd)),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    let (Some(parent), Some(name)) = (at.parent(), at.file_name()) else {
                        return None;
                    };
                    names.insert(name.to_owned());
                    at = parent;
                }
                Err(source) => {
                    let error = DaemonError::Watch { path: at.to_path_buf(), source };
                    tracing::warn!(error = %error, "a config directory is not watched; efr config reload still applies a change");
                    return None;
                }
            }
        }
    }
}

/// The file names that are the config file: the link's own and its target's.
pub(crate) fn names(file: &FileState) -> BTreeSet<OsString> {
    [Some(&file.path), file.symlink_target.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|path| path.file_name().map(ToOwned::to_owned))
        .collect()
}

/// The inotify instance that watches the config file's directories, and what it
/// watches.
#[derive(Debug)]
pub(crate) struct Watching {
    inotify: Inotify,
    watched: Watched,
}

/// Starts watching the config file of `state`, so a change made from now on is seen.
/// `None` when the watcher cannot start; `efr config reload` still works then.
pub(crate) async fn watch(state: &State) -> Option<Watching> {
    let path = state.dirs.config().join(CONFIG_FILE);
    let inotify = match Inotify::new() {
        Ok(inotify) => inotify,
        Err(source) => {
            let error = DaemonError::Watch { path, source };
            tracing::warn!(error = %error, "config changes are not watched; efr config reload still applies them");
            return None;
        }
    };
    // NOTE: a missing config root is created, so the first file in it is seen; it is
    // the daemon's own directory, and an editor creates it anyway.
    let root = state.dirs.config().to_path_buf();
    if let Ok(Err(error)) = tokio::task::spawn_blocking(move || create_private_dir(&root)).await {
        tracing::warn!(error = %error, "the config root could not be created");
    }
    let mut watched = Watched::default();
    watched.follow(&inotify, &file_state(&path).await);
    Some(Watching { inotify, watched })
}

/// Reloads when the config file changes, until `stop`.
pub(crate) async fn follow(state: Arc<State>, watching: Watching, stop: CancellationToken) {
    let Watching { inotify, mut watched } = watching;
    let path = state.dirs.config().join(CONFIG_FILE);
    let lost = |source: io::Error| {
        let error = DaemonError::Watch { path: path.clone(), source };
        tracing::warn!(error = %error, "config changes are no longer watched; efr config reload still applies them");
    };
    loop {
        let events = tokio::select! {
            () = stop.cancelled() => return,
            events = inotify.next() => events,
        };
        match events {
            Ok(events) if watched.concern(&events) => {}
            Ok(_) => continue,
            Err(source) => return lost(source),
        }
        let mut quiet = state.clock.sleep(DEBOUNCE);
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => return,
                () = quiet.as_mut() => break,
                more = inotify.next() => match more {
                    Ok(more) if watched.concern(&more) => quiet = state.clock.sleep(DEBOUNCE),
                    Ok(_) => {}
                    Err(source) => return lost(source),
                },
            }
        }
        // NOTE: the link is resolved and its directory watched before the reload reads
        // the file, so a change made after the read is always seen.
        watched.follow(&inotify, &file_state(&path).await);
        // The outcome is logged and kept for admin.status; a stopped task means the
        // daemon drains.
        if reload(&state, "file").await.is_err() {
            return;
        }
    }
}

/// Creates `dir` and its parents, the last with mode 0700, when it is missing.
fn create_private_dir(dir: &Path) -> Result<(), DaemonError> {
    use std::os::unix::fs::DirBuilderExt as _;
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|source| DaemonError::Io { path: dir.to_path_buf(), source })
}

#[cfg(test)]
mod tests;
