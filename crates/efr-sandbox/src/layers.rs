//! [`CacheLayers`]: the cache overlays of one call, which the launcher's layer helper
//! mounts after bwrap's setup.
//!
//! bwrap 0.13 cannot pass mount options to an overlay. An overlay in a user namespace
//! then falls back from the kernel's default `index=on` and `xino=auto`, and the kernel
//! logs two lines per overlay and call. With `index=on` an upper dir also stays "in
//! use" for a few milliseconds after the call's namespace dies, so the next call's
//! mount failed with `EBUSY`. So bwrap mounts no overlay. It mounts a private staging
//! dir in the masked runtime dir instead: a tmpfs, and in the `overlay` mode a bind of
//! each cache's layer dir in it. Then `efr-sbx layers` enters the call's user and mount
//! namespaces, mounts each overlay with `userxattr,index=off,xino=off`, moves the
//! plan's mounts inside the cache (masks, pins, floors) onto the overlay, so they
//! still win, and unmounts the staging dir. Only then does the inner stage go on.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::is_within;

/// The most bytes of the layer plan that the helper reads.
pub const MAX_LAYERS_BYTES: usize = 1024 * 1024;

/// The name of the staging dir in the launcher's dir inside the sandbox.
pub const STAGING_DIR: &str = "layers";

/// One cache overlay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CacheLayer {
    /// The user's cache: the lower layer and the mount point.
    pub target: PathBuf,
    /// The layer's dir in the staging dir: a dir of the staging tmpfs in the `tmp`
    /// mode, a bind of the conversation's layer dir in the `overlay` mode.
    pub dir: PathBuf,
    /// The upper layer, in `dir`.
    pub upper: PathBuf,
    /// The overlay's work dir, in `dir`.
    pub work: PathBuf,
    /// The mount points of the plan's mounts inside `target`, outermost only: the
    /// helper moves each one onto the overlay, with the mounts below it.
    pub moved: Vec<PathBuf>,
}

/// The cache overlays of one call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CacheLayers {
    /// The staging dir inside the sandbox; the helper unmounts it at the end.
    pub staging: PathBuf,
    /// True in the `tmp` mode: the helper makes each layer's dirs in the staging tmpfs,
    /// so the call's writes go away with it.
    pub fresh: bool,
    /// The overlays, in the order of the plan.
    pub layers: Vec<CacheLayer>,
}

impl CacheLayers {
    /// The plan as the bytes that the helper reads on its stdin.
    pub fn to_json(&self) -> Result<Vec<u8>, crate::SandboxError> {
        serde_json::to_vec(self)
            .map_err(|source| crate::SandboxError::Json { what: "layers", source })
    }

    /// Reads the helper's stdin.
    pub fn from_json(bytes: &[u8]) -> Result<CacheLayers, crate::SandboxError> {
        if bytes.len() > MAX_LAYERS_BYTES {
            return Err(crate::SandboxError::TooLarge {
                what: "layers",
                len: bytes.len(),
                max: MAX_LAYERS_BYTES,
            });
        }
        serde_json::from_slice(bytes)
            .map_err(|source| crate::SandboxError::Json { what: "layers", source })
    }
}

/// The outermost of `targets` that lie strictly inside `cache`, each once, in their
/// order: moving a mount moves every mount below it too.
pub(crate) fn moved_mounts<'a>(
    cache: &Path,
    targets: impl IntoIterator<Item = &'a Path>,
) -> Vec<PathBuf> {
    let inside: Vec<&Path> =
        targets.into_iter().filter(|target| *target != cache && is_within(target, cache)).collect();
    let mut out: Vec<PathBuf> = Vec::new();
    for target in &inside {
        let below_another = inside.iter().any(|other| other != target && is_within(target, other));
        if !below_another && !out.iter().any(|known| known == target) {
            out.push(target.to_path_buf());
        }
    }
    out
}

#[cfg(test)]
mod tests;
