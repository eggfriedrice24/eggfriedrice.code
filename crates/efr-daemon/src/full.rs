//! Full mode on the daemon's side: whether efrd lets a turn run in it, and the WARN
//! line when the config makes it the default.
//!
//! A turn runs in full mode only when `permissions.allow_full` is true and no refusal
//! of efrd holds ([`status`]). The conversations read the result at each resolve and
//! before each call (`settings.rs`), and `admin.status` reports the same value, so the
//! CLI shows what a turn would meet. The value follows the latest settings, so a reload
//! that turns undo off stops full mode from the next resolve on.
//!
//! [`FullTurns`] counts the full work that runs: a full turn from its first full call
//! to its end, and a job that a full call started until the job ends. While it runs, a
//! reload holds a change of `config.toml` that loosens permissions (`reload.rs`, the
//! config hold): a command of a full turn can write the file. It also keeps whether the
//! last reload turned full mode off ([`FullTurns::set_off`]): efrd then stops the jobs
//! of full calls (DV37), and a full job that starts after that stops at once.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_config::Settings;
use efr_protocol::{FullModeStatus, FullRefusal, Mode, TurnId};

use crate::jobs::JobKey;

/// The full work that runs now: the turns that made a full call and have not ended, and
/// the jobs that a full call started and that have not ended. Cheap to clone; clones
/// share the count.
#[derive(Debug, Clone, Default)]
pub(crate) struct FullTurns {
    work: Arc<Mutex<Work>>,
}

#[derive(Debug, Default)]
struct Work {
    turns: HashSet<TurnId>,
    jobs: HashSet<JobKey>,
    /// True after a reload installed settings that refuse full mode, until one allows it
    /// again.
    off: bool,
}

impl FullTurns {
    /// A full call of `turn` runs; the turn counts until [`end_turn`](Self::end_turn).
    pub(crate) fn begin_turn(&self, turn: TurnId) {
        self.lock().turns.insert(turn);
    }

    /// The turn `turn` ended, whatever its ending.
    pub(crate) fn end_turn(&self, turn: TurnId) {
        self.lock().turns.remove(&turn);
    }

    /// A full call started the job `job`; it counts until [`end_job`](Self::end_job).
    pub(crate) fn begin_job(&self, job: JobKey) {
        self.lock().jobs.insert(job);
    }

    /// The job `job` ended.
    pub(crate) fn end_job(&self, job: JobKey) {
        self.lock().jobs.remove(&job);
    }

    /// Marks full mode as turned off by a reload (`off`), or as allowed again.
    pub(crate) fn set_off(&self, off: bool) {
        self.lock().off = off;
    }

    /// True after a reload turned full mode off, until one allows it again.
    pub(crate) fn is_off(&self) -> bool {
        self.lock().off
    }

    /// True while a full turn or a job of one runs.
    pub(crate) fn running(&self) -> bool {
        let work = self.lock();
        !work.turns.is_empty() || !work.jobs.is_empty()
    }

    fn lock(&self) -> MutexGuard<'_, Work> {
        // Each change is one insert or one remove, so a poisoned lock still holds a
        // usable set.
        self.work.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether a turn may run in full mode with `settings` in an efrd that runs as root
/// when `root` is true: `permissions.allow_full`, and the first refusal that holds, in
/// this order: efrd runs as root, `snapshot.enabled` is false, `undo.enabled` is false,
/// `shell.program` is no zsh.
pub(crate) fn status(settings: &Settings, root: bool) -> FullModeStatus {
    FullModeStatus { allowed: settings.permissions.allow_full, refused: refusal(settings, root) }
}

/// The first reason why efrd refuses full mode with `settings`, or `None`.
fn refusal(settings: &Settings, root: bool) -> Option<FullRefusal> {
    if root {
        // NOTE: a command of a full turn would run as root with no question.
        return Some(FullRefusal::Root);
    }
    if !settings.snapshot.enabled {
        return Some(FullRefusal::SnapshotOff);
    }
    if !settings.undo.enabled {
        return Some(FullRefusal::UndoOff);
    }
    // NOTE: the test that gives a hidden shell the integration, which binds the key
    // that clears sudo's cache. With no program set, efrd looks for `zsh` on the
    // shells' `PATH`; a shell that falls back to another program fails each full call
    // in efr-shell, before its line.
    let zsh = settings.shell.program.as_deref().is_none_or(efr_shell::gets_integration);
    if !zsh {
        return Some(FullRefusal::NoZsh);
    }
    None
}

/// Writes one WARN line to the log when `settings` make full mode the default of every
/// turn that names no mode, at the start and at a reload that makes it so; with the
/// refusal of efrd when one holds, because those turns then run as `auto`.
pub(crate) fn warn_default(settings: &Settings, root: bool) {
    if settings.permissions.mode != Mode::Full {
        return;
    }
    match refusal(settings, root) {
        None => tracing::warn!(
            "permissions.mode is full: every turn that names no mode runs commands as you, with no sandbox and no questions"
        ),
        Some(refusal) => tracing::warn!(
            refusal = refusal.as_str(),
            "permissions.mode is full, but full mode cannot run: {}; turns that name no mode run as auto",
            refusal.describe()
        ),
    }
}

#[cfg(test)]
mod tests;
