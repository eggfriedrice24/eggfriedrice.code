//! The cache layers' collector (efr's auto spec, section 5.2): each conversation's
//! private upper layers in `$S/sandbox/<conversation>/cache` go after
//! `sandbox.cache_days` without a call, and when all of them pass
//! `sandbox.cache_max_gib`, the oldest conversation's go first, but never while a call
//! of that conversation runs. [`pick`] is the rule; `SandboxService::gc` runs it.
//!
//! A conversation's idle time counts from its [`LAST_CALL_FILE`], which each call's
//! plan touches: calls write below `cache/<name>/upper` and leave the time of `cache`
//! alone. A call counts as running while its tool call runs and, after that, while its
//! call dir has `started` and no `result.json` (a call left running at its timeout).
//! The layers go aside by one rename under the lock of the running calls, so a call
//! that starts while they are deleted gets new, empty ones.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use efr_sandbox::{RESULT_FILE, STARTED_FILE};

/// `$S/sandbox/<conversation>/last-call`: touched by each call's plan.
pub(crate) const LAST_CALL_FILE: &str = "last-call";
/// The prefix of layers set aside for deletion, next to `cache`.
const ASIDE_PREFIX: &str = "cache.gone-";

/// One conversation's cache layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layers {
    /// `$S/sandbox/<conversation>/cache`.
    pub(crate) dir: PathBuf,
    /// How long ago its last call ended.
    pub(crate) idle: Duration,
    /// Their size.
    pub(crate) bytes: u64,
    /// True while a call of the conversation runs.
    pub(crate) busy: bool,
}

/// The layers to delete: every idle one older than `max_idle`, then the oldest until
/// the rest fit in `max_bytes`. A busy conversation's layers stay.
pub(crate) fn pick(layers: &[Layers], max_idle: Duration, max_bytes: u64) -> Vec<PathBuf> {
    let mut keep: Vec<&Layers> = Vec::new();
    let mut gone = Vec::new();
    for layer in layers {
        if !layer.busy && layer.idle >= max_idle {
            gone.push(layer.dir.clone());
        } else {
            keep.push(layer);
        }
    }
    keep.sort_by_key(|layer| std::cmp::Reverse(layer.idle));
    let mut total: u64 = keep.iter().map(|layer| layer.bytes).sum();
    for layer in keep {
        if total <= max_bytes {
            break;
        }
        if !layer.busy {
            total = total.saturating_sub(layer.bytes);
            gone.push(layer.dir.clone());
        }
    }
    gone
}

/// The cache layers below `sandbox_root` (`$S/sandbox`), as of `now`; `busy` says
/// whether a conversation's call runs. It blocks.
pub(crate) fn scan(
    sandbox_root: &Path,
    now: SystemTime,
    busy: &dyn Fn(&str) -> bool,
) -> Vec<Layers> {
    let Ok(entries) = std::fs::read_dir(sandbox_root) else { return Vec::new() };
    let mut layers = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let dir = entry.path().join("cache");
        let Ok(meta) = std::fs::symlink_metadata(&dir) else { continue };
        if !meta.is_dir() {
            continue;
        }
        let stamp = std::fs::metadata(entry.path().join(LAST_CALL_FILE))
            .and_then(|stamp| stamp.modified())
            .ok();
        let used = match (meta.modified().ok(), stamp) {
            (Some(dir), Some(stamp)) => dir.max(stamp),
            (dir, stamp) => dir.or(stamp).unwrap_or(now),
        };
        layers.push(Layers {
            idle: now.duration_since(used).unwrap_or_default(),
            bytes: size(&dir),
            busy: busy(&name),
            dir,
        });
    }
    layers
}

/// True when a call dir below `shell_dir` (`$R/sbx/<conversation>`) has `started` and
/// no `result.json`: its launcher still runs, also when its tool call returned at the
/// timeout. It blocks.
pub(crate) fn launcher_running(shell_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(shell_dir) else { return false };
    entries.flatten().any(|entry| {
        let call = entry.path();
        call.join(STARTED_FILE).exists() && !call.join(RESULT_FILE).exists()
    })
}

/// Moves the layers `dir` aside, so a call that starts after this gets new ones, and
/// returns where they went. The caller holds the lock of the running calls and has
/// checked that none of the conversation runs. It blocks.
pub(crate) fn set_aside(dir: &Path, now: SystemTime) -> Option<PathBuf> {
    let stamp = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let aside = dir.with_file_name(format!("{ASIDE_PREFIX}{stamp}"));
    match std::fs::rename(dir, &aside) {
        Ok(()) => Some(aside),
        Err(error) => {
            tracing::warn!(dir = %dir.display(), error = %error, "cache layers could not be set aside");
            None
        }
    }
}

/// The layers below `sandbox_root` that an earlier collection set aside and could not
/// delete. It blocks.
pub(crate) fn left_aside(sandbox_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(sandbox_root) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
        .flat_map(|inner| inner.flatten())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(ASIDE_PREFIX))
        .map(|entry| entry.path())
        .collect()
}

/// The bytes of the files below `dir`, links not followed.
fn size(dir: &Path) -> u64 {
    let mut total = 0_u64;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                todo.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Deletes `dirs`. Their files are the sandbox's copies, owned by the user; a dir
/// that cannot go is logged and tried again at the next look. It blocks.
pub(crate) fn remove(dirs: &[PathBuf]) {
    for dir in dirs {
        // NOTE: an overlay's work dir holds a directory without permissions; make it
        // readable before the removal.
        make_removable(dir);
        if let Err(error) = std::fs::remove_dir_all(dir) {
            tracing::warn!(dir = %dir.display(), error = %error, "cache layers could not be removed");
        } else {
            tracing::info!(dir = %dir.display(), "removed idle cache layers");
        }
    }
}

fn make_removable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&dir) else { continue };
        if !meta.is_dir() {
            continue;
        }
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        if let Ok(entries) = std::fs::read_dir(&dir) {
            todo.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
}

#[cfg(test)]
mod tests;
