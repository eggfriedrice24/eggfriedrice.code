//! The cache layers' collector (efr's auto spec, section 5.2): each conversation's
//! private upper layers in `$S/sandbox/<conversation>/cache` go after
//! `sandbox.cache_days` without a call, and when all of them pass
//! `sandbox.cache_max_gib`, the oldest conversation's go first, but never while a call
//! of that conversation runs. [`pick`] is the rule; `SandboxService::gc` runs it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
        let used = meta.modified().unwrap_or(now);
        layers.push(Layers {
            idle: now.duration_since(used).unwrap_or_default(),
            bytes: size(&dir),
            busy: busy(&name),
            dir,
        });
    }
    layers
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
