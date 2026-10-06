//! The turn-end surface report (efr's auto spec, section 5.8): the files that a turn
//! changed through the launcher and that run code later outside the sandbox, such as
//! `build.rs`, a `package.json` with scripts or a `.cargo/config.toml` with a
//! `build.rustc-wrapper`.
//!
//! Before phase 4 there is no snapshot, so efrd takes the hardened `git status` of each
//! project root before the turn's first call through the launcher there, and compares
//! it with the same status at the turn's end: a file that matches
//! `sandbox.surface_files` and is new in the status, or changed size or time, goes into
//! the report, with what in it runs code. The surface guard's changes of the turn come
//! with it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use efr_protocol::{ReportedFile, SurfaceChange, TurnId};
use efr_sandbox::{name_matches, report_file};
use efr_scope::{Git, Home};
use jiff::Timestamp;

use crate::sandbox::facts::HARDENED;
use crate::sandbox::fs::DaemonFs;

/// How long one status for the report may take.
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);

/// The most entries of one status that the report reads.
const MAX_ENTRIES: usize = 10_000;

/// The most bytes of a file that the report reads for what in it runs code.
const MAX_CONTENT: usize = 256 * 1024;

/// What a file looked like: its size and its change time, in nanoseconds.
type Fingerprint = (u64, i128);

/// The status entries of one root that match the patterns, by path below the root.
type Snapshot = BTreeMap<PathBuf, Fingerprint>;

/// What efrd keeps of one turn.
#[derive(Debug)]
struct TurnTrack {
    /// When the turn's first call was judged.
    started: Timestamp,
    /// Each project root with its status before the turn's first launcher call there.
    roots: Vec<(PathBuf, Snapshot)>,
    /// The surface guard's changes of the turn's calls.
    changes: Vec<SurfaceChange>,
}

/// The turns that efrd follows for the report, and when each started.
#[derive(Debug, Default)]
pub(crate) struct Turns {
    turns: Mutex<HashMap<TurnId, TurnTrack>>,
}

impl Turns {
    /// When `turn` started, as efrd first saw it at `now`.
    pub(crate) fn started(&self, turn: TurnId, now: Timestamp) -> Timestamp {
        let mut turns = self.lock();
        turns
            .entry(turn)
            .or_insert_with(|| TurnTrack { started: now, roots: Vec::new(), changes: Vec::new() })
            .started
    }

    /// True when the report already holds the status of `root` for `turn`.
    fn has_root(&self, turn: TurnId, root: &Path) -> bool {
        self.lock()
            .get(&turn)
            .is_some_and(|track| track.roots.iter().any(|(known, _)| known == root))
    }

    /// Takes the status of each of `roots` that `turn` has not looked at yet, before a
    /// call through the launcher.
    pub(crate) async fn before_call(
        &self,
        turn: TurnId,
        now: Timestamp,
        roots: &[PathBuf],
        patterns: &[String],
        git: &Git,
        home: &Home,
    ) {
        self.started(turn, now);
        for root in roots {
            if self.has_root(turn, root) {
                continue;
            }
            let snapshot = snapshot(root, patterns, git, home).await.unwrap_or_default();
            if let Some(track) = self.lock().get_mut(&turn) {
                track.roots.push((root.clone(), snapshot));
            }
        }
    }

    /// Keeps the surface guard's `changes` of a call of `turn`.
    pub(crate) fn changes(&self, turn: TurnId, changes: &[SurfaceChange]) {
        if let Some(track) = self.lock().get_mut(&turn) {
            track.changes.extend_from_slice(changes);
        }
    }

    /// The report of `turn`, which ends now; efrd forgets the turn.
    pub(crate) async fn finish(
        &self,
        turn: TurnId,
        patterns: &[String],
        git: &Git,
        home: &Home,
    ) -> Vec<ReportedFile> {
        let Some(track) = self.lock().remove(&turn) else { return Vec::new() };
        let mut files: Vec<ReportedFile> = Vec::new();
        for (root, before) in &track.roots {
            let Some(after) = snapshot(root, patterns, git, home).await else { continue };
            for (relative, fingerprint) in &after {
                if before.get(relative) == Some(fingerprint) {
                    continue;
                }
                let content = DaemonFs::read(&root.join(relative), MAX_CONTENT).unwrap_or_default();
                files.push(report_file(relative, &content));
            }
        }
        for change in &track.changes {
            let path = track
                .roots
                .iter()
                .find_map(|(root, _)| change.path.strip_prefix(root).ok())
                .map_or_else(|| change.path.clone(), Path::to_path_buf);
            let detail = Some(change.what());
            let file = ReportedFile { path, detail };
            if !files.contains(&file) {
                files.push(file);
            }
        }
        files
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<TurnId, TurnTrack>> {
        // Each entry is replaced whole, so a poisoned lock still holds usable ones.
        self.turns.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// True when `relative`, a path below a root, matches one of `patterns`: a pattern
/// with a `/` matches the path's last components, one without matches the file name;
/// `*` matches any text.
pub(crate) fn matches_surface(relative: &Path, patterns: &[String]) -> bool {
    let parts: Vec<String> =
        relative.iter().map(|part| part.to_string_lossy().into_owned()).collect();
    patterns.iter().any(|pattern| {
        let wanted: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
        if wanted.is_empty() || wanted.len() > parts.len() {
            return false;
        }
        let tail = &parts[parts.len() - wanted.len()..];
        wanted.iter().zip(tail).all(|(pattern, part)| name_matches(pattern, part))
    })
}

/// The hardened `git status` of `root`: each changed or untracked file that matches
/// `patterns`, with its fingerprint. `None` when git did not answer.
async fn snapshot(root: &Path, patterns: &[String], git: &Git, home: &Home) -> Option<Snapshot> {
    let mut args: Vec<&str> = HARDENED.to_vec();
    args.extend(["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames"]);
    let git = git.clone().with_timeout(STATUS_TIMEOUT);
    let listed = git.run(root, home, args).await.ok()??;
    let mut snapshot = Snapshot::new();
    for entry in listed.split(|byte| *byte == 0).take(MAX_ENTRIES) {
        // NOTE: each entry is `XY path`; a deleted file has nothing to report.
        let Some(path) = entry.get(3..) else { continue };
        if entry.starts_with(b" D") || entry.starts_with(b"D") || path.is_empty() {
            continue;
        }
        let relative = PathBuf::from(String::from_utf8_lossy(path).into_owned());
        if !matches_surface(&relative, patterns) {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(root.join(&relative)) {
            let changed = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| i128::try_from(since.as_nanos()).unwrap_or(i128::MAX));
            snapshot.insert(relative, (meta.len(), changed));
        }
    }
    Some(snapshot)
}

#[cfg(test)]
mod tests;
