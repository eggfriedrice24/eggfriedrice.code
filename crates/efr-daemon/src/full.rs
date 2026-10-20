//! Full mode on the daemon's side: whether efrd lets a turn run in it, and the WARN
//! line when the config makes it the default.
//!
//! A turn runs in full mode only when `permissions.allow_full` is true and no refusal
//! of efrd holds ([`status`]). The conversations read the result at each resolve and
//! before each call (`settings.rs`), and `admin.status` reports the same value, so the
//! CLI shows what a turn would meet. The value follows the latest settings, so a reload
//! that turns undo off stops full mode from the next resolve on.

use efr_config::Settings;
use efr_protocol::{FullModeStatus, FullRefusal, Mode};

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
