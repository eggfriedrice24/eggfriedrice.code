//! [`CacheDeletes`]: the paths in the user's tool caches that one call deleted in its
//! private upper layers (efr's auto spec, section 5.2, phase A).
//!
//! A delete in a cache overlay leaves the lower file as it was: the overlay only puts
//! a whiteout, a character device 0/0, with the same name in the upper layer. So "free
//! some disk space" freed nothing. After the call, the launcher walks each upper layer
//! through [`LayerView`] and lists the whiteouts. efrd then asks whether to delete the
//! same paths in the real cache.
//!
//! The rules of the walk:
//!
//! - A whiteout counts only when the lower entry of its name exists. Contained code can
//!   make a whiteout with `mknod`, so a whiteout for a name that the cache does not have
//!   reports nothing.
//! - A path at or below a pin, a mask or a mount that the plan moved onto the overlay
//!   never counts, and neither does a whiteout of a directory that holds one. A cache
//!   at or below a mask lists nothing.
//! - An opaque directory (deleted and made again) hides the whole lower directory. Its
//!   delete would remove every lower entry below it, also the ones that the call made
//!   again in its own layer, so the walk only counts it.
//! - A name that is not UTF-8 cannot travel in `result.json`, so the walk counts it.
//! - The walk stops at [`DeleteLimits`]: a number of paths and bytes, which keep
//!   `result.json` far below its own limit, and a number of entries that it reads.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::SandboxSummary;

use crate::fs_view::{FileKind, FsView};
use crate::layers::CacheLayer;
use crate::paths::is_within;

/// The most entries of the upper layers that one walk reads.
pub const MAX_DELETE_ENTRIES: usize = 65_536;

/// What one entry of an upper layer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LayerEntry {
    /// A directory. `opaque` when the overlay marked it (`user.overlay.opaque`): it
    /// hides every lower entry below it.
    Dir {
        /// True for an opaque directory.
        opaque: bool,
    },
    /// A whiteout: a character device 0/0 that hides the lower entry of its name.
    Whiteout,
    /// Anything else: a file, a link, another device.
    Other,
}

/// Read access to one upper layer, by paths relative to its root. The implementation in
/// `efr-sbx` reads a directory that the layer helper opened, with no symbolic link on
/// the way.
pub trait LayerView {
    /// The entries of the directory `dir` (empty for the root), without `.` and `..`,
    /// in any order.
    fn entries(&self, dir: &Path) -> io::Result<Vec<(OsString, LayerEntry)>>;
}

/// The limits of one walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteLimits {
    /// The most paths that the list holds.
    pub max_paths: usize,
    /// The most bytes that the paths take in `result.json`.
    pub max_bytes: usize,
    /// The most entries of the upper layers that the walk reads.
    pub max_entries: usize,
}

impl Default for DeleteLimits {
    fn default() -> Self {
        DeleteLimits {
            max_paths: SandboxSummary::MAX_CACHE_DELETES,
            max_bytes: SandboxSummary::MAX_CACHE_DELETE_BYTES,
            max_entries: MAX_DELETE_ENTRIES,
        }
    }
}

/// The cache deletes of one call so far: the listed paths, and how many deletes the
/// list leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheDeletes {
    limits: DeleteLimits,
    paths: Vec<PathBuf>,
    more: u32,
    bytes: usize,
    entries: usize,
}

impl CacheDeletes {
    /// An empty list with `limits` for every layer that it walks.
    pub fn new(limits: DeleteLimits) -> Self {
        CacheDeletes { limits, paths: Vec::new(), more: 0, bytes: 0, entries: 0 }
    }

    /// Walks the upper layer `upper` of `layer`, whose lower layer `lower` reads, and
    /// adds its deletes. `skip` holds the pins and masks of the spec; the mounts that
    /// the plan moved onto the overlay come from `layer`.
    pub fn walk(
        &mut self,
        layer: &CacheLayer,
        skip: &[PathBuf],
        upper: &dyn LayerView,
        lower: &dyn FsView,
    ) {
        if skip.iter().any(|skip| is_within(&layer.target, skip)) {
            return;
        }
        let skipped: Vec<&Path> = skip
            .iter()
            .chain(&layer.moved)
            .map(PathBuf::as_path)
            .filter(|path| is_within(path, &layer.target))
            .collect();
        let mut todo = vec![PathBuf::new()];
        while let Some(dir) = todo.pop() {
            if self.entries >= self.limits.max_entries {
                self.more = self.more.saturating_add(1);
                return;
            }
            let mut entries = match upper.entries(&dir) {
                Ok(entries) => entries,
                Err(_) => {
                    self.more = self.more.saturating_add(1);
                    continue;
                }
            };
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (name, entry) in entries {
                self.entries += 1;
                if self.entries > self.limits.max_entries {
                    self.more = self.more.saturating_add(1);
                    return;
                }
                if name.to_str().is_none() {
                    self.more = self.more.saturating_add(1);
                    continue;
                }
                let relative = dir.join(&name);
                let path = layer.target.join(&relative);
                if skipped.iter().any(|skip| is_within(&path, skip)) {
                    continue;
                }
                let holds_skipped = skipped.iter().any(|skip| is_within(skip, &path));
                match entry {
                    LayerEntry::Whiteout if lower.lstat(&path).is_ok() => {
                        if holds_skipped {
                            self.more = self.more.saturating_add(1);
                        } else {
                            self.push(path);
                        }
                    }
                    LayerEntry::Dir { opaque: true } => {
                        if matches!(lower.lstat(&path), Ok(FileKind::Dir)) {
                            self.more = self.more.saturating_add(1);
                        }
                    }
                    // NOTE: a directory that the lower layer lacks is new, and nothing
                    // below it can hide a lower entry.
                    LayerEntry::Dir { opaque: false }
                        if matches!(lower.lstat(&path), Ok(FileKind::Dir)) =>
                    {
                        todo.push(relative);
                    }
                    _ => {}
                }
            }
        }
    }

    fn push(&mut self, path: PathBuf) {
        let bytes = path.to_str().map_or(usize::MAX, json_len);
        let fits = self.paths.len() < self.limits.max_paths
            && self.bytes.saturating_add(bytes) <= self.limits.max_bytes;
        if fits {
            self.bytes += bytes;
            self.paths.push(path);
        } else {
            self.more = self.more.saturating_add(1);
        }
    }

    /// The listed paths, in the order of the walk.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// How many deletes the list leaves out.
    pub fn more(&self) -> u32 {
        self.more
    }

    /// Puts the list into `summary`, as `result.json` carries it.
    pub fn fill(self, summary: &mut SandboxSummary) {
        summary.cache_deletes = self.paths;
        summary.cache_deletes_more = self.more;
    }
}

/// The bytes that `text` takes as a JSON string in a list: its quotes, its escapes and
/// a comma.
fn json_len(text: &str) -> usize {
    let escaped: usize = text
        .chars()
        .map(|char| match char {
            '"' | '\\' => 2,
            char if u32::from(char) < 0x20 => 6,
            char => char.len_utf8(),
        })
        .sum();
    escaped + 3
}

#[cfg(test)]
mod tests;
