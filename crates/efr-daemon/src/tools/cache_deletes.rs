//! The cache deletes that the classifier or the user allowed, applied to the user's real
//! tool caches (efr's auto spec, section 5.2, phase A).
//!
//! The launcher lists what a contained call deleted in its private cache layers, and
//! the turn asks the engine and then the classifier. efrd does not trust the list: it
//! checks each path again against its own caches, pins and masks, from the latest
//! settings, before it deletes anything. One path that fails the check refuses the
//! whole list.

use std::path::{Path, PathBuf};

use efr_config::Settings;
use efr_conversation::CacheDeleteOutcome;
use efr_permissions::sandbox_masks;
use efr_sandbox::{CacheOverlay, expand_home, is_normal, is_within};

use crate::sandbox::fs::DaemonFs;

/// What the model reads when efrd refuses a list.
pub(crate) const REFUSED: &str = "efr did not delete these paths in the real cache: a path is \
     not in a tool cache, or it is a pin, a mask or a directory that holds one. The deletes \
     stayed private to the sandbox.";

/// One delete that passed the check: the cache and the path below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checked {
    pub(crate) cache: PathBuf,
    pub(crate) relative: PathBuf,
}

/// The caches of `settings` with their pins, under `home`.
pub(crate) fn caches(home: &Path, settings: &Settings) -> Vec<CacheOverlay> {
    // NOTE: the layer dirs do not matter here; only the targets and the pins do.
    settings
        .sandbox
        .caches
        .iter()
        .map(|cache| expand_home(cache, home))
        .filter(|cache| cache.is_absolute())
        .map(|cache| CacheOverlay::new(&cache, home, home))
        .collect()
}

/// The masks that efrd never deletes or copies: the engine's secrets, the sandbox-only
/// masks and `sandbox.mask`.
pub(crate) fn masks(home: &Path, settings: &Settings, secrets: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut masks = secrets;
    masks.extend(sandbox_masks(home));
    masks.extend(settings.sandbox.mask.iter().map(|mask| expand_home(mask, home)));
    masks
}

/// Each of `paths` with its cache, or the first path that is not a normal path strictly
/// inside a cache, or that lies at, below or above a pin or a mask.
pub(crate) fn check(
    paths: &[PathBuf],
    caches: &[CacheOverlay],
    masks: &[PathBuf],
) -> Result<Vec<Checked>, PathBuf> {
    let pins: Vec<&PathBuf> = caches.iter().flat_map(|cache| &cache.pins).collect();
    let mut checked = Vec::with_capacity(paths.len());
    for path in paths {
        let cache = caches
            .iter()
            .filter(|cache| path != &cache.target && is_within(path, &cache.target))
            .max_by_key(|cache| cache.target.as_os_str().len());
        let guarded = |guard: &PathBuf| is_within(path, guard) || is_within(guard, path);
        let refused = !is_normal(path)
            || cache.is_none()
            || pins.iter().copied().any(guarded)
            || masks.iter().any(guarded);
        match cache {
            Some(cache) if !refused => {
                let relative = path.strip_prefix(&cache.target).unwrap_or(path).to_path_buf();
                checked.push(Checked { cache: cache.target.clone(), relative });
            }
            _ => return Err(path.clone()),
        }
    }
    Ok(checked)
}

/// Deletes the checked paths in the real caches. A path that is gone already counts as
/// nothing. `Err` when the first delete failed and nothing went; a later failure ends
/// the list with what went before it. It blocks.
pub(crate) fn apply(checked: &[Checked]) -> Result<CacheDeleteOutcome, String> {
    let mut outcome = CacheDeleteOutcome::default();
    for delete in checked {
        let path = delete.cache.join(&delete.relative);
        match DaemonFs::remove_beneath(&delete.cache, &delete.relative) {
            Ok(Some(freed)) => {
                outcome.freed_bytes = outcome.freed_bytes.saturating_add(freed);
                outcome.deleted.push(path);
            }
            Ok(None) => {}
            Err(error) if outcome.deleted.is_empty() => {
                return Err(format!(
                    "efr could not delete {} in the real cache: {error}. The deletes stayed \
                     private to the sandbox.",
                    path.display()
                ));
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error, "a cache delete failed; the rest stays");
                break;
            }
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests;
