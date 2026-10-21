//! [`InnerPolicy`]: what `efr-sbx run` sends to `efr-sbx inner` on the policy fd.
//!
//! The inner stage runs inside bwrap's namespaces as trusted code in an untrusted
//! place. It reads this policy, moves the terminal copy to fd 2 and the records pipe
//! to fd 3, closes every other descriptor, applies Landlock, then seccomp, and executes
//! the child shell. It reports its own failure as a `setup-error` record on fd 3. For a
//! proxy plan it starts the bridge first, before its own Landlock domain, for a job
//! with port forwards the forwarder, and for a call with a reach the reach helper.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::landlock::LandlockPolicy;
use crate::plan::MountPlan;
use crate::seccomp::SeccompProfile;
use crate::spec::{ForwardPlan, ReachPlan};

/// The most bytes of a policy.
pub const MAX_POLICY_BYTES: usize = 1024 * 1024;

/// The fd that the child sees the records pipe on.
pub const RECORDS_FD: i32 = 3;

/// Everything the inner stage needs, as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct InnerPolicy {
    /// The Landlock rule set.
    pub landlock: LandlockPolicy,
    /// The seccomp filter.
    pub seccomp: SeccompProfile,
    /// The descriptor of the records pipe's write end, moved to [`RECORDS_FD`].
    pub records_fd: i32,
    /// The descriptor of the terminal copy, moved to fd 2 so the child's stderr is the
    /// terminal again while bwrap's own stderr went to the launcher.
    pub terminal_fd: Option<i32>,
    /// The child: `zsh -f <inside>/child.zsh`.
    pub argv: Vec<OsString>,
    /// The handshake with the launcher's layer helper, when the plan has cache
    /// overlays: the inner stage waits until they are in place.
    pub layers_sync: Option<LayersSync>,
    /// The start dir, which the inner stage enters after the overlays are in place:
    /// with overlays, bwrap starts in `/`.
    pub cwd: Option<PathBuf>,
    /// The bridge's socket inside the call, `$XRT/efr-proxy.sock`: `Some` starts the
    /// bridge (`efr-sbx bridge`) before the inner stage's own Landlock domain, on
    /// `127.0.0.1:3128` of the call's network namespace. Only a proxy plan sets it.
    #[serde(default)]
    pub proxy_socket: Option<PathBuf>,
    /// The port forwards of a job: `Some` starts the forwarder (`efr-sbx forward`)
    /// before the inner stage's own Landlock domain. Only a job with a forward plan in a
    /// network namespace of its own sets it.
    #[serde(default)]
    pub forward: Option<InnerForward>,
    /// The reach of a contained foreground call: `Some` starts the reach helper
    /// (`efr-sbx reach`) before the inner stage's own Landlock domain. Only a call with
    /// a reach plan in a network namespace of its own sets it.
    #[serde(default)]
    pub reach: Option<InnerReach>,
}

/// What the inner stage passes to the reach helper of a call: the call's reach plan,
/// with the socket as the call sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InnerReach {
    /// The call's reach socket inside, `$XRT/efr-reach.sock`.
    pub socket: PathBuf,
    /// [`ReachPlan::ports`]: the ports that the helper binds before the child runs.
    pub ports: Vec<u16>,
    /// [`ReachPlan::proxy`].
    pub proxy: bool,
}

impl InnerReach {
    /// The helper's view of `plan`, with the socket at `socket` inside.
    pub fn new(plan: &ReachPlan, socket: PathBuf) -> InnerReach {
        InnerReach { socket, ports: plan.ports.clone(), proxy: plan.proxy }
    }
}

/// What the inner stage passes to the forwarder of a job: the job's forward plan, with
/// the socket as the job sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InnerForward {
    /// The job's forward socket inside, `$XRT/efr-forward.sock`.
    pub socket: PathBuf,
    /// [`ForwardPlan::poll_ms`].
    pub poll_ms: u32,
    /// [`ForwardPlan::stable_ms`].
    pub stable_ms: u32,
    /// [`ForwardPlan::ephemeral_stable_ms`].
    pub ephemeral_stable_ms: u32,
    /// [`ForwardPlan::max_ports`].
    pub max_ports: u8,
    /// [`ForwardPlan::proxy`].
    pub proxy: bool,
}

impl InnerForward {
    /// The forwarder's view of `plan`, with the socket at `socket` inside.
    pub fn new(plan: &ForwardPlan, socket: PathBuf) -> InnerForward {
        InnerForward {
            socket,
            poll_ms: plan.poll_ms,
            stable_ms: plan.stable_ms,
            ephemeral_stable_ms: plan.ephemeral_stable_ms,
            max_ports: plan.max_ports,
            proxy: plan.proxy,
        }
    }

    /// The forward plan as the forwarder sees it: the socket is the one inside. The
    /// forwarder gives it to [`Debounce::new`](crate::Debounce::new).
    pub fn plan(&self) -> ForwardPlan {
        ForwardPlan {
            socket: self.socket.clone(),
            poll_ms: self.poll_ms,
            stable_ms: self.stable_ms,
            ephemeral_stable_ms: self.ephemeral_stable_ms,
            max_ports: self.max_ports,
            proxy: self.proxy,
        }
    }
}

/// The two pipes between the inner stage and the launcher while the launcher's helper
/// mounts the cache overlays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayersSync {
    /// The write end on which the inner stage sends its process id, as the host's
    /// `/proc` names it, and a newline: bwrap's setup is done.
    pub ready_fd: i32,
    /// The read end on which the launcher sends one `g` when the overlays are in
    /// place. Anything else, or the end of the pipe, stops the call.
    pub go_fd: i32,
}

impl InnerPolicy {
    /// The policy of `plan`, with the descriptors the launcher passes. A call that
    /// keeps the host's network namespace for its network view gets the network view's
    /// seccomp profile; every other call gets phase 1's.
    pub fn new(plan: &MountPlan, records_fd: i32, terminal_fd: Option<i32>) -> InnerPolicy {
        let seccomp =
            if plan.net_view() { SeccompProfile::net_view() } else { SeccompProfile::phase1() };
        InnerPolicy {
            landlock: plan.landlock(),
            seccomp,
            records_fd,
            terminal_fd,
            argv: plan.child_argv().to_vec(),
            layers_sync: None,
            cwd: None,
            proxy_socket: plan.proxy_socket().map(Path::to_path_buf),
            forward: plan.forward().cloned(),
            reach: plan.reach().cloned(),
        }
    }

    /// The policy as the bytes of the policy pipe.
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec(self).map_err(|source| SandboxError::Json { what: "policy", source })
    }

    /// Reads the policy pipe.
    pub fn from_json(bytes: &[u8]) -> Result<InnerPolicy, SandboxError> {
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(SandboxError::TooLarge {
                what: "policy",
                len: bytes.len(),
                max: MAX_POLICY_BYTES,
            });
        }
        serde_json::from_slice(bytes)
            .map_err(|source| SandboxError::Json { what: "policy", source })
    }
}
