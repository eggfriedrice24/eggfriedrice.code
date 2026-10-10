//! The cache deletes of a contained call: `efr_sandbox::CacheDeletes` over the upper
//! layers that the layer helper passed back (efr's auto spec, section 5.2, phase A).
//!
//! The layer helper opens each upper dir to mount it, and before it exits it sends the
//! descriptors to the launcher over its stdout, a Unix socket. In the `tmp` mode no
//! path leads to an upper once the helper unmounted its staging dir, and the open
//! descriptor keeps that tmpfs alive after the overlays go. In the `overlay` mode the
//! same descriptor names the conversation's upper dir. So the launcher reads both modes
//! the same way, after the call, while it still holds the conversation's layer lock.

use std::ffi::OsString;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use efr_sandbox::{CacheDeletes, CacheLayers, DeleteLimits, LayerEntry, LayerView, SandboxSpec};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, ResolveFlags};

use crate::real_fs::RealFs;

/// The xattr with which an overlay marks an opaque directory when it runs with
/// `userxattr`, as the layer helper mounts it.
const OPAQUE_XATTR: &str = "user.overlay.opaque";

/// The bytes of a buffer that one directory listing reads into.
const LISTING_BUFFER: usize = 16 * 1024;

/// One upper layer, read through the descriptor of its root: every path below it opens
/// with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`.
#[derive(Debug)]
pub(crate) struct UpperView {
    root: OwnedFd,
}

impl UpperView {
    pub(crate) fn new(root: OwnedFd) -> Self {
        UpperView { root }
    }

    fn open(&self, dir: &Path) -> io::Result<OwnedFd> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        let resolve = ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS;
        Ok(rustix::fs::openat2(&self.root, dir, flags, Mode::empty(), resolve)?)
    }
}

impl LayerView for UpperView {
    fn entries(&self, dir: &Path) -> io::Result<Vec<(OsString, LayerEntry)>> {
        let fd = self.open(dir)?;
        let mut buffer = vec![std::mem::MaybeUninit::<u8>::uninit(); LISTING_BUFFER];
        let mut listing = rustix::fs::RawDir::new(&fd, &mut buffer);
        let mut entries = Vec::new();
        while let Some(entry) = listing.next() {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let kind = match entry.file_type() {
                FileType::Unknown => rustix::fs::statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW)
                    .map(|stat| FileType::from_raw_mode(stat.st_mode))
                    .unwrap_or(FileType::Unknown),
                kind => kind,
            };
            let entry_kind = match kind {
                FileType::Directory => LayerEntry::Dir { opaque: opaque(&fd, name) },
                FileType::CharacterDevice => {
                    match rustix::fs::statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                        Ok(stat) if stat.st_rdev == 0 => LayerEntry::Whiteout,
                        _ => LayerEntry::Other,
                    }
                }
                _ => LayerEntry::Other,
            };
            let name = std::os::unix::ffi::OsStringExt::from_vec(name.to_bytes().to_vec());
            entries.push((name, entry_kind));
        }
        Ok(entries)
    }
}

/// True when the directory `name` in `dir` carries the overlay's opaque mark.
fn opaque(dir: &OwnedFd, name: &std::ffi::CStr) -> bool {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let Ok(sub) = rustix::fs::openat(dir, name, flags, Mode::empty()) else { return false };
    let mut value = [0_u8; 4];
    matches!(rustix::fs::fgetxattr(&sub, OPAQUE_XATTR, &mut value[..]), Ok(1) if value[0] == b'y')
}

/// The cache deletes of a call whose overlays `layers` describes, with `uppers` the
/// descriptors of their upper layers in the same order. Nothing when the helper sent
/// another number of them.
pub(crate) fn list(spec: &SandboxSpec, layers: &CacheLayers, uppers: Vec<OwnedFd>) -> CacheDeletes {
    let mut deletes = CacheDeletes::new(DeleteLimits::default());
    if uppers.len() != layers.layers.len() {
        return deletes;
    }
    let skip: Vec<PathBuf> = spec
        .caches
        .iter()
        .flat_map(|cache| cache.pins.iter().cloned())
        .chain(spec.masks.iter().map(|mask| mask.path.clone()))
        .collect();
    for (layer, upper) in layers.layers.iter().zip(uppers) {
        deletes.walk(layer, &skip, &UpperView::new(upper), &RealFs);
    }
    deletes
}

#[cfg(test)]
mod tests;
