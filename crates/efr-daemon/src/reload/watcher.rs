//! The config file watcher: reloads when `config.toml` changes.
//!
//! It watches the config root and, when `config.toml` is a symbolic link, the directory
//! of the file the link resolves to, so an edit in a dotfiles repository reloads too.
//! Directories are watched, not the file: an editor that saves by writing a new file
//! and renaming it over the old one (vim, nvim) replaces the file the watch would hold.
//! A burst of events (a save is often several) becomes one reload after
//! [`DEBOUNCE`] of quiet, timed by the injected clock. After each reload the link is
//! resolved again and the watched directories follow it. A removed file reloads as no
//! file, as at start, and a file created again reloads as itself.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_config::{CONFIG_FILE, FileState};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::reload::{file_state, reload};
use crate::state::State;

/// The quiet after the last event before the reload starts.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(200);

/// Events that may wait for the task. One waiting event is enough to reload, so a full
/// queue loses nothing.
const EVENTS: usize = 64;

/// The directories under watch and the file names in them that are the config file.
#[derive(Debug, Default)]
pub(crate) struct Watched {
    dirs: BTreeSet<PathBuf>,
    names: BTreeSet<OsString>,
}

impl Watched {
    /// True when `event` may change the config file: it names one of the file's names,
    /// or the watcher lost track and asks for a fresh look.
    pub(crate) fn concerns(&self, event: &notify::Result<notify::Event>) -> bool {
        match event {
            Ok(event) => {
                event.need_rescan()
                    || event
                        .paths
                        .iter()
                        .any(|path| path.file_name().is_some_and(|name| self.names.contains(name)))
            }
            // NOTE: an error may hide a change; a reload of an unchanged file is cheap.
            Err(_) => true,
        }
    }

    /// Watches the directories of `file` and stops watching the ones it no longer has.
    fn follow(&mut self, watcher: &mut RecommendedWatcher, file: &FileState) {
        let wanted: BTreeSet<PathBuf> = file.watched_dirs().into_iter().collect();
        for dir in self.dirs.difference(&wanted) {
            // A directory that was removed is no longer watched anyway.
            let _ = watcher.unwatch(dir);
        }
        let mut dirs = BTreeSet::new();
        for dir in wanted {
            if self.dirs.contains(&dir) {
                dirs.insert(dir);
                continue;
            }
            match watcher.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    dirs.insert(dir);
                }
                Err(source) => {
                    let error = DaemonError::Watch { path: dir, source };
                    tracing::warn!(error = %error, "a config directory is not watched; efr config reload still applies a change");
                }
            }
        }
        self.dirs = dirs;
        self.names = names(file);
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

/// A watcher that watches the config file's directories, and the events it sends.
#[derive(Debug)]
pub(crate) struct Watching {
    watcher: RecommendedWatcher,
    watched: Watched,
    events: mpsc::Receiver<notify::Result<notify::Event>>,
}

/// Starts watching the config file of `state`, so a change made from now on is seen.
/// `None` when the watcher cannot start; `efr config reload` still works then.
pub(crate) async fn watch(state: &State) -> Option<Watching> {
    let path = state.dirs.config().join(CONFIG_FILE);
    let (sender, events) = mpsc::channel(EVENTS);
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = sender.try_send(event);
    });
    let mut watcher = match watcher {
        Ok(watcher) => watcher,
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
    watched.follow(&mut watcher, &file_state(&path).await);
    Some(Watching { watcher, watched, events })
}

/// Reloads when the config file changes, until `stop`.
pub(crate) async fn follow(state: Arc<State>, watching: Watching, stop: CancellationToken) {
    let Watching { mut watcher, mut watched, mut events } = watching;
    let path = state.dirs.config().join(CONFIG_FILE);
    loop {
        let event = tokio::select! {
            () = stop.cancelled() => return,
            event = events.recv() => event,
        };
        let Some(event) = event else {
            return;
        };
        if !watched.concerns(&event) {
            continue;
        }
        let mut quiet = state.clock.sleep(DEBOUNCE);
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => return,
                () = quiet.as_mut() => break,
                more = events.recv() => match more {
                    None => return,
                    Some(more) if watched.concerns(&more) => quiet = state.clock.sleep(DEBOUNCE),
                    Some(_) => {}
                },
            }
        }
        // The outcome is logged and kept for admin.status; a stopped task means the
        // daemon drains.
        if reload(&state, "file").await.is_err() {
            return;
        }
        watched.follow(&mut watcher, &file_state(&path).await);
    }
}

/// Creates `dir` and its parents, the last with mode 0700, when it is missing.
fn create_private_dir(dir: &std::path::Path) -> Result<(), DaemonError> {
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
