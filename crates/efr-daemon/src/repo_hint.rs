//! The hint in a git repository that is not a registered project (DV53): a prompt from
//! a terminal outside every project, inside a git work tree, gets one line that says
//! writes there ask the user and how to register the repository. efr never registers
//! one by itself.
//!
//! The hint shows at most once per repository in a rolling day, read from efrd's clock
//! (a test's `TestClock`), so no time zone counts. efrd keeps when it last showed each
//! one in `<state root>/repo-hints.json` (mode 0600, written by an atomic rename), so a
//! restart does not show it again. Each write drops the entries older than a day, and
//! so does the read at start.
//!
//! On disk: one JSON object, the work tree's root to the time of the last hint, such as
//! `{"/home/u/p/tool": "2026-10-20T09:12:00Z"}`. A root that is not UTF-8 is kept in
//! memory only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use jiff::Timestamp;

/// The file's name in the state root.
pub(crate) const FILE: &str = "repo-hints.json";

/// How long a repository gets no second hint.
pub(crate) const QUIET: Duration = Duration::from_secs(24 * 3600);

/// When efrd last showed the hint for each repository.
#[derive(Debug)]
pub(crate) struct RepoHints {
    file: PathBuf,
    shown: Mutex<BTreeMap<PathBuf, Timestamp>>,
}

impl RepoHints {
    /// The hints of the file in the state root `state`, as it holds them at `now`. A
    /// missing or broken file holds none. It blocks; async callers run it on the
    /// blocking pool.
    pub(crate) fn load(state: &Path, now: Timestamp) -> RepoHints {
        let file = state.join(FILE);
        let mut shown = read(&file);
        shown.retain(|_, at| recent(*at, now));
        RepoHints { file, shown: Mutex::new(shown) }
    }

    /// True when the repository at `root` got no hint in the last day before `now`.
    pub(crate) fn due(&self, root: &Path, now: Timestamp) -> bool {
        let shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
        shown.get(root).is_none_or(|at| !recent(*at, now))
    }

    /// Records the hint for the repository at `root` at `now`, drops the entries older
    /// than a day, and returns the file and its new text, for [`write`].
    pub(crate) fn shown(&self, root: PathBuf, now: Timestamp) -> (PathBuf, Vec<u8>) {
        let mut shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
        shown.insert(root, now);
        shown.retain(|_, at| recent(*at, now));
        (self.file.clone(), text(&shown))
    }
}

/// True when `at` is less than a day before `now`, or after it.
fn recent(at: Timestamp, now: Timestamp) -> bool {
    Duration::try_from(now.duration_since(at)).map_or(true, |since| since < QUIET)
}

/// The hints that the file at `file` holds; none when it is missing or broken.
fn read(file: &Path) -> BTreeMap<PathBuf, Timestamp> {
    let Ok(bytes) = std::fs::read(file) else {
        return BTreeMap::new();
    };
    match serde_json::from_slice::<BTreeMap<String, Timestamp>>(&bytes) {
        Ok(entries) => entries.into_iter().map(|(root, at)| (PathBuf::from(root), at)).collect(),
        Err(error) => {
            tracing::warn!(error = %error, file = %file.display(), "the repository hints could not be read; every repository may get its hint again");
            BTreeMap::new()
        }
    }
}

/// The file's text for `shown`.
fn text(shown: &BTreeMap<PathBuf, Timestamp>) -> Vec<u8> {
    let entries: BTreeMap<&str, &Timestamp> =
        shown.iter().filter_map(|(root, at)| Some((root.to_str()?, at))).collect();
    serde_json::to_vec_pretty(&entries).unwrap_or_default()
}

/// Writes `text` to `file`, which [`RepoHints::shown`] returned. A failed write is
/// logged: the hint may then show again after a restart. It blocks.
pub(crate) fn write(file: &Path, text: &[u8]) {
    if let Some(dir) = file.parent()
        && let Err(error) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(error = %error, "the repository hints could not be written");
        return;
    }
    if let Err(error) = efr_stdx::fs::write_atomic(file, text) {
        tracing::warn!(error = %error, "the repository hints could not be written");
    }
}

#[cfg(test)]
mod tests;
