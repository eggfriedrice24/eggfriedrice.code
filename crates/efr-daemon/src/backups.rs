//! The legacy backup store of floor files (efr's auto spec, section 10.9, phase A):
//! `$XDG_DATA_HOME/efr/file-backups/<conversation>/<turn>/<n>/`. From phase B efrd
//! writes nothing here: the home store of `efr-snapshot` holds the copies (`home.rs`,
//! plan PB28). The name `backups/` holds the copies of the database.
//!
//! [`collect`] still deletes the old copies by the keys of the snapshot store: it keeps
//! the newest `snapshot.keep_turns` turns of each conversation, drops a turn older than
//! `snapshot.max_age_days`, and removes the store's directory once it is empty. efr
//! does not move the old copies into the home store.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use efr_protocol::{ConversationId, TurnId};

/// The store's directory under the data root.
pub(crate) const FILE_BACKUPS_DIR: &str = "file-backups";

/// Removes the backups of all but the newest `keep_turns` turns of each conversation,
/// and those older than `max_age` (`snapshot.keep_turns`, `snapshot.max_age_days`).
/// The snapshot collector of `run.rs` calls it at each round, with the time of the
/// daemon's clock.
pub(crate) async fn collect(dir: &Path, keep_turns: usize, max_age: Duration, now: SystemTime) {
    let dir = dir.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || collect_at(&dir, keep_turns, max_age, now));
    match removed.await {
        Ok(0) => {}
        Ok(removed) => tracing::debug!(removed, "collected old floor file backups"),
        Err(_) => tracing::warn!("collecting the floor file backups panicked"),
    }
}

/// [`collect`] at the time `now`: returns how many turn dirs it removed. A turn's time is
/// the time in its id (a version 7 UUID), else the time its dir last changed. A name
/// that is not an id of efr stays. It blocks.
pub(crate) fn collect_at(
    dir: &Path,
    keep_turns: usize,
    max_age: Duration,
    now: SystemTime,
) -> usize {
    let oldest = now.checked_sub(max_age).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    let Ok(conversations) = fs::read_dir(dir) else { return 0 };
    for conversation in conversations.flatten() {
        let path = conversation.path();
        let is_id = conversation
            .file_name()
            .to_str()
            .is_some_and(|name| name.parse::<ConversationId>().is_ok());
        if !is_id || !conversation.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(turns) = fs::read_dir(&path) else { continue };
        let mut turns: Vec<(SystemTime, PathBuf)> = turns
            .flatten()
            .filter(|turn| turn.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|turn| {
                let id = turn.file_name().to_str()?.parse::<TurnId>().ok()?;
                let time = turn_time(&id).or_else(|| turn.metadata().ok()?.modified().ok())?;
                Some((time, turn.path()))
            })
            .collect();
        turns.sort_by(|left, right| right.cmp(left));
        for (n, (time, turn)) in turns.iter().enumerate() {
            if n < keep_turns && *time >= oldest {
                continue;
            }
            match fs::remove_dir_all(turn) {
                Ok(()) => removed += 1,
                Err(error) => {
                    tracing::warn!(dir = %turn.display(), error = %error, "old floor file backups could not be removed");
                }
            }
        }
        // NOTE: fails while the conversation keeps a turn, which is the point.
        let _ = fs::remove_dir(&path);
    }
    // NOTE: the same for the store itself: nothing writes it any more.
    let _ = fs::remove_dir(dir);
    removed
}

/// The time in the version 7 UUID of `turn`, when it has one.
fn turn_time(turn: &TurnId) -> Option<SystemTime> {
    let (seconds, nanos) = turn.as_uuid().get_timestamp()?.to_unix();
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

#[cfg(test)]
mod tests;
