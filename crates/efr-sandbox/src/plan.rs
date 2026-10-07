//! [`MountPlan`]: the ordered mounts of one contained call, checked before any launch.
//!
//! The rules (the spec's sections 3.3 and 5):
//!
//! - Mounts are sorted by target depth, shallow first; at one depth a mask comes
//!   first, then a cache overlay, a write root, a widening and a floor last, so a floor
//!   wins over a widening at the same depth.
//! - A mask always wins over a floor: a floor at or below a mask is dropped, so a
//!   secret that is also a tool config (`~/.npmrc`) stays empty.
//! - Masks and floors follow every link and apply to the real target; a floor that is
//!   reached through a link inside a write root, which a call could change, refuses
//!   the call, and so does a git dir to pin that is a link.
//! - A write root or a widening is never `/`, the home directory or above it, and never
//!   lies at or inside a mask (except scratch below efr's data root) or a floor.
//! - A mask or a floor that does not exist is skipped; a floor that lies in no
//!   writable place is skipped too, because everything else is read-only already.
//! - Only the top `.git` of a project root is pinned, with its config and hooks
//!   read-only; a `.git` file is a floor. Nested repositories are left to the surface
//!   guard, so a call can still delete a repository that an earlier call made.
//! - Every directory of the hidden shell's `PATH` that lies in a write root is a
//!   floor, and a relative `PATH` entry refuses the call.
//! - A cache overlay is a [`CacheLayers`] entry that the launcher's helper mounts after
//!   bwrap's setup; the plan's mounts inside the cache move onto it, so they still win.
//!   A cache with another mount at its own path gets no overlay: that mount covers it.
//!   In the `overlay` mode two caches never share a layer dir.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::layers::CacheLayers;
use crate::spec::{FloorKind, MaskKind, WriteRootKind};

mod build;
mod explain;

pub use explain::{Explanation, StartDir};

/// The order of mounts at one depth: a mask first, a floor last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OpKind {
    /// Hides content: a tmpfs or `/dev/null`.
    Mask,
    /// A cache overlay.
    Overlay,
    /// A write root, the private tmp, a pin.
    WriteRoot,
    /// An approved widening of this call.
    Widening,
    /// A read-only bind: a floor or a launcher file.
    Floor,
}

/// Why a mount is in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MountOrigin {
    /// The user's runtime dir or efr's runtime root: every socket, the bus, efr's own.
    Runtime,
    /// efr's data or state root.
    EfrState,
    /// A mask of the spec.
    Mask(MaskKind),
    /// A cache overlay.
    Cache,
    /// A write root of the spec, or scratch.
    WriteRoot(WriteRootKind),
    /// The private `/tmp` or `/var/tmp`.
    PrivateTmp,
    /// The private `/dev/shm`.
    SharedMemory,
    /// A pinned git dir: a mount point that cannot be renamed or removed.
    Pin,
    /// A floor.
    Floor(FloorKind),
    /// A file of the launcher inside the sandbox.
    Asset,
    /// A socket or bus grant.
    Socket,
    /// A device grant.
    Device,
}

/// One mount of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MountOp {
    /// `--perms <perms> --tmpfs <target>`.
    Tmpfs {
        /// The mount point.
        target: PathBuf,
        /// The mode of the tmpfs root, such as `0o500`.
        perms: u32,
    },
    /// `--ro-bind-data <fd> <target>` with a descriptor that reads as empty: a masked
    /// file, which reads as empty and refuses writes.
    DevNull {
        /// The masked file.
        target: PathBuf,
    },
    /// A cache overlay with the conversation's layer as its upper dir; the launcher's
    /// helper mounts it (`CacheLayers`).
    Overlay {
        /// The user's cache, read-only below.
        lower: PathBuf,
        /// The conversation's upper layer.
        upper: PathBuf,
        /// The overlay's work dir.
        work: PathBuf,
        /// The mount point, the same path as `lower`.
        target: PathBuf,
    },
    /// A cache overlay with a new upper dir for each call; the launcher's helper mounts
    /// it (`CacheLayers`).
    TmpOverlay {
        /// The user's cache.
        lower: PathBuf,
        /// The mount point.
        target: PathBuf,
    },
    /// `--bind-fd` or `--ro-bind-fd` with a descriptor that the launcher opens on
    /// `source` with no symbolic link on the way.
    Bind {
        /// What is bound, outside the sandbox.
        source: PathBuf,
        /// Where, inside.
        target: PathBuf,
        /// `--bind-fd` when true, else `--ro-bind-fd`.
        writable: bool,
    },
    /// `--dev-bind <node> <node>`: one device of a grant.
    DevBind {
        /// The device node.
        node: PathBuf,
    },
}

impl MountOp {
    /// Where the mount lands inside the sandbox.
    pub fn target(&self) -> &Path {
        match self {
            MountOp::Tmpfs { target, .. }
            | MountOp::DevNull { target }
            | MountOp::Overlay { target, .. }
            | MountOp::TmpOverlay { target, .. }
            | MountOp::Bind { target, .. } => target,
            MountOp::DevBind { node } => node,
        }
    }
}

/// One mount with its place in the order and its reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mount {
    /// What bwrap does.
    pub op: MountOp,
    /// Its rank at one depth.
    pub kind: OpKind,
    /// Why.
    pub origin: MountOrigin,
}

/// Something the plan left out, for `efr sandbox check` and the logs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlanNote {
    /// A mask that does not exist.
    MissingMask(PathBuf),
    /// A floor that does not exist; the surface guard covers new names.
    MissingFloor(PathBuf),
    /// A floor at or below a mask: the mask wins.
    FloorUnderMask {
        /// The floor.
        floor: PathBuf,
        /// The mask.
        mask: PathBuf,
    },
    /// A write root that does not exist or is not a directory.
    MissingRoot(PathBuf),
    /// A cache that does not exist, lies in a write root or is masked.
    CacheSkipped(PathBuf),
    /// An unmask grant that matches no mask.
    UnmaskUnused(PathBuf),
}

/// The checked, ordered mounts of one contained call, and what Landlock and the
/// environment need from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPlan {
    pub(crate) mounts: Vec<Mount>,
    pub(crate) notes: Vec<PlanNote>,
    pub(crate) unshare_net: bool,
    pub(crate) write_dirs: Vec<PathBuf>,
    pub(crate) write_files: Vec<PathBuf>,
    pub(crate) resolve_unix: Vec<PathBuf>,
    pub(crate) devices: Vec<PathBuf>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) inside_launcher: PathBuf,
    pub(crate) child_argv: Vec<OsString>,
    pub(crate) private_tmp: PathBuf,
    pub(crate) layers: Option<CacheLayers>,
    pub(crate) layer_sources: Vec<PathBuf>,
}

impl MountPlan {
    /// The mounts after the three fixed ones (`/` and `/proc` read-only, a new
    /// `/dev`), in the order bwrap applies them.
    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    /// What the plan left out.
    pub fn notes(&self) -> &[PlanNote] {
        &self.notes
    }

    /// True when the call gets a network namespace of its own (no
    /// `Grant::OpenNetwork`).
    pub fn unshares_network(&self) -> bool {
        self.unshare_net
    }

    /// Variables the launcher sets after the environment filter: the address of a
    /// granted bus.
    pub fn env_overrides(&self) -> &[(String, String)] {
        &self.env
    }

    /// The command bwrap runs inside the sandbox before `efr-sbx inner` takes over:
    /// the launcher's path inside.
    pub fn inside_launcher(&self) -> &Path {
        &self.inside_launcher
    }

    /// The child shell that `efr-sbx inner` executes after Landlock and seccomp:
    /// `zsh -f <inside>/child.zsh`.
    pub fn child_argv(&self) -> &[OsString] {
        &self.child_argv
    }

    /// The directory outside that holds the private `/tmp`.
    pub fn private_tmp(&self) -> &Path {
        &self.private_tmp
    }

    /// The writable directories as the write grants, roots, caches and the private
    /// tmp give them, without one that lies in another.
    pub fn write_dirs(&self) -> &[PathBuf] {
        &self.write_dirs
    }

    /// The single files that a write grant on a file makes writable.
    pub fn write_files(&self) -> &[PathBuf] {
        &self.write_files
    }

    /// The cache overlays that `efr-sbx layers` mounts after bwrap's setup; `None`
    /// when the call has none. bwrap itself mounts no overlay.
    pub fn cache_layers(&self) -> Option<&CacheLayers> {
        self.layers.as_ref()
    }
}

#[cfg(test)]
mod tests;
