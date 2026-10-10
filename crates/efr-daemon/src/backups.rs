//! The backup store of floor files (efr's auto spec, section 10.9, phase A): before
//! efrd lets a call edit a file that runs code later, such as `~/.zshrc`, it copies the
//! file to `$XDG_DATA_HOME/efr/file-backups/<conversation>/<turn>/<n>/`, so the user
//! can copy it back. The name `backups/` holds the copies of the database.
//!
//! The copy before a call and the collector come in the next change; until then the
//! store stays empty and [`collect`] has nothing to remove.

use std::path::Path;
use std::time::Duration;

/// The store's directory under the data root.
pub(crate) const FILE_BACKUPS_DIR: &str = "file-backups";

/// Removes the backups of all but the newest `keep_turns` turns of each conversation,
/// and those older than `max_age` (`snapshot.keep_turns`, `snapshot.max_age_days`).
/// The snapshot collector of `run.rs` calls it at each round.
pub(crate) async fn collect(dir: &Path, keep_turns: usize, max_age: Duration) {
    tracing::trace!(dir = %dir.display(), keep_turns, ?max_age, "no floor file backups to collect yet");
}
