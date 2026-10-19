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
//! - `$SCRATCH/jobs`, where efrd writes the output of each job, is a floor of every
//!   call once efrd made it. A job (the spec has a `JobPlan`) also gets each git dir
//!   that it finds at its start as a floor whole: the pinned ones and the nested ones
//!   that the surface guard's scan finds in the guard roots.
//! - A cache overlay is a [`CacheLayers`] entry that the launcher's helper mounts after
//!   bwrap's setup; the plan's mounts inside the cache move onto it, so they still win.
//!   A cache with another mount at its own path gets no overlay: that mount covers it.
//!   Nor does a cache in `/tmp`, `/var/tmp` or `/dev`, which are private inside.
//!   In the `overlay` mode two caches never share a layer dir.
//! - A proxy plan binds the call's socket read-only at `$XRT/efr-proxy.sock` for the
//!   bridge, with no `RESOLVE_UNIX` rule for the child, and sets the proxy variables. It
//!   always keeps the private network namespace.
//! - A socket grant never binds the system bus, the session bus, a socket of a service
//!   manager or a container engine ([`SERVICE_SOCKETS`]), a socket below
//!   `$XRT/systemd/`, or the control socket of a desktop. The plan checks the path after
//!   every link, so a link in a write root to the session bus binds nothing. Only a bus
//!   grant binds a bus, and only a desktop grant binds a desktop's socket.
//! - A filtered bus (`Grant::BusFilter`) binds the call's own socket read-only at
//!   `/run/dbus/system_bus_socket`, over an empty tmpfs on `/run/dbus`, or at
//!   `$XRT/bus`, with a `RESOLVE_UNIX` rule for the child on exactly that place. It
//!   never joins `Grant::Bus` of the same bus or the network view.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::layers::CacheLayers;
use crate::spec::{FloorKind, MaskKind, WriteRootKind};

mod build;
mod explain;

pub use explain::{Explanation, StartDir};

/// The port of the bridge inside a call with a proxy plan, on `127.0.0.1` of the call's
/// own network namespace.
pub const PROXY_PORT: u16 = 3128;

/// The proxy address that the variables of a proxy call name: the bridge.
pub const PROXY_URL: &str = "http://127.0.0.1:3128";

/// The hosts that tools reach directly in a proxy call: servers of the call itself.
pub const NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// The variables that a proxy call gets, so that every tool uses the bridge: the
/// generic names in both cases and the names of cargo, npm, yarn and pip, which win
/// over a proxy in the user's own tool config.
pub const PROXY_VARIABLES: &[&str] = &[
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "https_proxy",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "CARGO_HTTP_PROXY",
    "npm_config_proxy",
    "npm_config_https_proxy",
    "YARN_HTTP_PROXY",
    "YARN_HTTPS_PROXY",
    "PIP_PROXY",
];

/// The variables that a proxy call gets for the hosts that skip the proxy.
pub const NO_PROXY_VARIABLES: &[&str] = &["NO_PROXY", "no_proxy"];

/// The sockets of service managers and container engines, each as the last components
/// of its path, which no socket grant binds: through one, a call starts a unit or a
/// container that runs outside the sandbox. A copy of `efr_permissions::SERVICE_SOCKETS`
/// (this crate has no edge to it); a test in efr-daemon keeps the two lists equal.
pub const SERVICE_SOCKETS: &[&str] = &[
    "systemd/private",
    "systemd/io.systemd.Manager",
    "docker.sock",
    "podman/podman.sock",
    "containerd.sock",
    "containerd/containerd.sock",
    "crio.sock",
    "crio/crio.sock",
    "buildkit/buildkitd.sock",
    "lxd/unix.socket",
    "incus/unix.socket",
    "libvirt/libvirt-sock",
    "libvirt/virtqemud-sock",
];

/// The system bus socket at its fixed place, which no socket grant binds, whatever the
/// spec names as the system bus.
pub const SYSTEM_BUS_SOCKET: &str = "/run/dbus/system_bus_socket";

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
    pub(crate) net_view: bool,
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
    pub(crate) proxy_socket: Option<PathBuf>,
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
    /// `Grant::OpenNetwork` and no network view).
    pub fn unshares_network(&self) -> bool {
        self.unshare_net
    }

    /// True when the call keeps the host's network namespace for `Grant::NetView`
    /// (no `Grant::OpenNetwork`, no proxy): the inner stage then installs
    /// `SeccompProfile::net_view()`, and Landlock also handles the TCP rights.
    pub fn net_view(&self) -> bool {
        self.net_view
    }

    /// Variables the launcher sets after the environment filter: the address of a
    /// granted bus, and for a proxy plan [`PROXY_VARIABLES`], [`NO_PROXY_VARIABLES`]
    /// and `NODE_USE_ENV_PROXY`.
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

    /// The layer dir of each overlay that keeps its private layer, which bwrap binds
    /// into the staging dir: every overlay in the `overlay` mode, the kept caches in the
    /// `tmp` mode (`CacheOverlay::keep`), none in the `readonly` mode.
    pub fn layer_dirs(&self) -> &[PathBuf] {
        &self.layer_sources
    }

    /// The inside path of the proxy socket of a `NetworkPlan::Proxy` call,
    /// `$XRT/efr-proxy.sock`: the inner stage starts the bridge on it. `None` for every
    /// other call, and never for a call that keeps the host's network namespace.
    pub fn proxy_socket(&self) -> Option<&Path> {
        self.proxy_socket.as_deref()
    }
}

#[cfg(test)]
mod tests;
