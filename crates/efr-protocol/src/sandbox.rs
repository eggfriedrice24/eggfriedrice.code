//! The `auto` sandbox on the wire: how a shell call runs ([`Launch`]), what an approved
//! exit opens ([`Grant`]), the exits themselves, the probe's status, what a contained
//! call reports back, and the classifier's record and verdict.
//!
//! These are wire types because events, the engine, the tools, the daemon and the view
//! all name them, and this crate is the one place that every one of them can reach.
//! The logic that uses them lives elsewhere: the mount plan in `efr-sandbox`, the
//! prediction of exits in `efr-permissions`, the questions in `efr-conversation`.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Mode;

pub(crate) mod exit;
pub(crate) mod record;
pub(crate) mod status;
pub(crate) mod summary;

/// How a shell call runs.
///
/// On the wire: `{"kind": "direct"}`, `{"kind": "contained", "grants": [...]}` or
/// `{"kind": "unsandboxed"}`. `grants` is left out when empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Launch {
    /// The line is typed into the hidden shell, as in `manual` and `cautious`.
    Direct,
    /// The line runs in the kernel sandbox (bubblewrap, Landlock and seccomp), widened by
    /// exactly these grants for this one call.
    Contained {
        /// The approved widenings; none for a routine call.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        grants: Vec<Grant>,
    },
    /// The line runs in the exit child, outside the sandbox, with the user's full
    /// rights: an approved privilege, persistence, upload or outside exit.
    Unsandboxed,
}

impl Launch {
    /// A contained launch with no grant: the routine case of `auto`.
    pub const fn contained() -> Self {
        Launch::Contained { grants: Vec::new() }
    }

    /// The grants of a contained launch; none for the other launches.
    pub fn grants(&self) -> &[Grant] {
        match self {
            Launch::Contained { grants } => grants,
            Launch::Direct | Launch::Unsandboxed => &[],
        }
    }

    /// True when the call goes through the launcher (`efr-sbx run`), contained or not.
    pub const fn uses_launcher(&self) -> bool {
        matches!(self, Launch::Contained { .. } | Launch::Unsandboxed)
    }
}

/// One widening of the sandbox for one call, after the user (or, from phase 3, the
/// classifier) approved an exit. A grant never lifts a mask of an engine secret and
/// never crosses a floor.
///
/// On the wire an object with a `kind` member, such as
/// `{"kind": "write", "path": "/home/u/notes"}` or `{"kind": "open_network"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Grant {
    /// A writable bind of this one path.
    Write {
        /// The path, absolute.
        path: PathBuf,
    },
    /// TCP to one host through the proxy (phase 2).
    Host {
        /// The host name, in lower case.
        host: String,
        /// The port.
        port: u16,
    },
    /// No network namespace for this call: the host's network. Masks, write roots,
    /// `no_new_privs` and the socket rules stay.
    OpenNetwork,
    /// A bind of one Unix socket and the right to connect to it.
    Socket {
        /// The socket, absolute.
        path: PathBuf,
    },
    /// A bind of one message bus socket and the right to connect to it.
    Bus {
        /// Which bus.
        bus: BusKind,
    },
    /// A bind of one device node with read, write and ioctl rights.
    Device {
        /// The node, below `/dev`.
        path: PathBuf,
    },
    /// One read mask removed. Never for an engine secret.
    Unmask {
        /// The masked path, absolute.
        path: PathBuf,
    },
}

impl Grant {
    /// The path the grant names, for the grants that name one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Grant::Write { path }
            | Grant::Socket { path }
            | Grant::Device { path }
            | Grant::Unmask { path } => Some(path),
            Grant::Host { .. } | Grant::OpenNetwork | Grant::Bus { .. } => None,
        }
    }
}

/// A D-Bus message bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BusKind {
    /// The system bus, `/run/dbus/system_bus_socket`.
    System,
    /// The user's session bus, below `$XDG_RUNTIME_DIR`.
    Session,
}

/// How the sandbox treats the tool caches (`~/.cargo`, `~/.cache` and the others of
/// `sandbox.caches`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CacheMode {
    /// An overlay per conversation: the sandbox reads the user's cache and writes to a
    /// private upper layer that the user's own builds never see.
    #[default]
    Overlay,
    /// A tmpfs upper layer per call: writes vanish when the call ends.
    Tmp,
    /// The caches stay read-only.
    Readonly,
}

impl CacheMode {
    /// Every mode, in the order the docs list them.
    pub const ALL: [CacheMode; 3] = [CacheMode::Overlay, CacheMode::Tmp, CacheMode::Readonly];

    /// The wire and config form, such as `overlay`.
    pub const fn as_str(self) -> &'static str {
        match self {
            CacheMode::Overlay => "overlay",
            CacheMode::Tmp => "tmp",
            CacheMode::Readonly => "readonly",
        }
    }
}

/// What network a contained call has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NetworkMode {
    /// Loopback only (phase 1). A network need is a `host` exit.
    #[default]
    None,
    /// TCP through the efr proxy to the hosts of the allow list (phase 2).
    Proxy,
}

impl NetworkMode {
    /// The wire form, such as `none`.
    pub const fn as_str(self) -> &'static str {
        match self {
            NetworkMode::None => "none",
            NetworkMode::Proxy => "proxy",
        }
    }
}

/// Why a turn that asked for one mode runs with a stricter one: `auto` without a
/// working sandbox, or in a project at the home directory, runs as `cautious`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ModeFallback {
    /// The mode that the prompt or the config asked for.
    pub asked: Mode,
    /// Why it was not used, in one sentence, such as `Landlock ABI 6 found; auto needs
    /// 9 (Linux 7.1)`.
    pub reason: String,
}

impl ModeFallback {
    /// The reason of an `auto` turn in a registered project at the home directory or
    /// above it, which is never a write root, so the turn runs as `cautious`.
    pub const HOME_PROJECT_REASON: &str = "auto cannot use your home directory as a project";

    /// The reason of an `auto` turn when the probe says the sandbox is unavailable but
    /// gives no reason of its own.
    pub const UNAVAILABLE_REASON: &str = "the sandbox is not available";
}

#[cfg(test)]
mod tests;
