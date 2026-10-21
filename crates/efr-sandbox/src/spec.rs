//! [`SandboxSpec`]: everything one call's launch needs, as plain paths and names.
//!
//! efrd builds the spec from the turn, the settings, the registry, its own roots and
//! the engine's secret tables, and writes it to `$CALL/spec.json` (0600, in a 0700
//! dir of the masked runtime root). `efr-sbx run` reads it, checks it and builds the
//! [`MountPlan`](crate::MountPlan). This crate never reads a config file or a secret
//! table itself, so the engine and the sandbox cannot disagree about a secret.

use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, CacheMode, CallId, ConversationId, DesktopKind, Grant};
use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::forward::{BRIDGE_PORT, MAX_DAEMON_FORWARDS, MAX_JOB_FORWARDS, MIN_PORT};
use crate::paths::is_normal;

/// The version of the spec that this build writes and reads. Version 2 adds
/// [`SandboxSpec::job`], version 3 [`JobPlan::forward`], version 4
/// [`SandboxSpec::reach`]: efrd and its launcher copy come from one build, and a stale
/// copy refuses the spec.
pub const SPEC_VERSION: u32 = 4;

/// The most ports of a call's reach plan: as many as efrd forwards for all its jobs
/// ([`MAX_DAEMON_FORWARDS`]).
pub const MAX_REACH_PORTS: usize = MAX_DAEMON_FORWARDS;

/// The hard file size limit of a job (`RLIMIT_FSIZE`, soft and hard): 4 GiB.
pub const JOB_FSIZE_BYTES: u64 = 4 << 30;

/// The seconds that efrd adds to a job's limit for the launcher's own deadline
/// ([`JobPlan::backstop_secs`]): efrd's timer ends the job first.
pub const JOB_BACKSTOP_SECS: u64 = 60;

/// The `oom_score_adj` of a job's launcher, which every process of the job inherits:
/// the highest, so the kernel's OOM killer picks a job before the user's own programs.
/// A process may raise its own value without privilege.
pub const JOB_OOM_SCORE_ADJ: i32 = 1000;

/// The dir of jobs: `shell_dir/jobs/<call>` holds a job's call dir, and
/// `sandbox_dir/jobs/<call>` its cache layers and its guard files. `$SCRATCH/jobs`
/// holds the job outputs that efrd writes.
pub const JOB_DIR: &str = "jobs";

/// The name of a call's proxy socket in its call dir: efrd binds `$CALL/proxy.sock`,
/// and [`NetworkPlan::Proxy`] must name exactly that file.
pub const PROXY_SOCKET_FILE: &str = "proxy.sock";

/// The name of a job's forward socket in its call dir: efrd binds `$CALL/forward.sock`,
/// and [`ForwardPlan::socket`] must name exactly that file.
pub const FORWARD_SOCKET_FILE: &str = "forward.sock";

/// The name of a call's reach socket in its call dir: efrd binds `$CALL/reach.sock`,
/// and [`ReachPlan::socket`] must name exactly that file.
pub const REACH_SOCKET_FILE: &str = "reach.sock";

/// The dir of a call's filtered bus sockets in its call dir: `$CALL/bus/` (phase E). It
/// holds nothing else, and the bus proxy may make and remove files only there.
pub const BUS_FILTER_DIR: &str = "bus";

/// The file name of `bus`'s filtered socket in the bus dir: `system.sock` or
/// `session.sock`. The names are relative, so the proxy binds them in its working
/// dir, and a deep call dir never meets the 107 bytes of a socket address.
pub fn bus_filter_file(bus: BusKind) -> &'static str {
    // NOTE: `BusKind` has two buses; the plan refuses a bus that this build does not
    // know before it asks for a path.
    if bus == BusKind::System { "system.sock" } else { "session.sock" }
}

/// The highest nice value.
const MAX_NICE: u8 = 19;

/// The most bytes a `spec.json` may have.
pub const MAX_SPEC_BYTES: usize = 1024 * 1024;

/// One call's launch: what may be written, what is hidden, what stays read-only, what
/// an approval widened, and where the launcher finds its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxSpec {
    /// [`SPEC_VERSION`].
    pub version: u32,
    /// The conversation; `runtime.shell_dir` ends with it.
    pub conversation: ConversationId,
    /// The call; `runtime.call_dir` ends with it.
    pub call: CallId,
    /// Contained, or the exit child.
    pub launch: SpecLaunch,
    /// The write roots: the turn's project, the named projects, the git dirs of a
    /// worktree project, scratch and the user's extra roots. Scratch, the private
    /// `/tmp` and the grants are added by the plan too.
    pub write_roots: Vec<WriteRoot>,
    /// The tool caches, mounted as overlays.
    pub caches: Vec<CacheOverlay>,
    /// How the caches are mounted.
    pub cache_mode: CacheMode,
    /// Read masks: engine secrets, sandbox masks, project `.env` files, the user's.
    /// The plan adds efr's own data, state and runtime roots.
    pub masks: Vec<Mask>,
    /// Floors: read-only even inside a write root. The plan adds the git surface of
    /// each project root and the `PATH` dirs that lie in a write root.
    pub floors: Vec<Floor>,
    /// Git dirs to pin besides the top `.git` of each project root: the git dir and
    /// common dir of a worktree project, from efrd's registration record.
    pub git_dirs: Vec<PathBuf>,
    /// The write roots that the surface guard scans for git dirs after the call.
    pub guard_roots: Vec<PathBuf>,
    /// Names that the surface guard reports when a call creates them at a repository
    /// root, such as `.envrc` (`efr_permissions::protected_names`).
    pub protected_names: Vec<String>,
    /// The widenings of this call.
    pub grants: Vec<Grant>,
    /// The network of the call.
    pub network: NetworkPlan,
    /// The environment lists.
    pub env: EnvPlan,
    /// The hidden shell's `PATH`. `efr-sbx run` replaces it with its own `PATH`
    /// before it builds the plan, because it runs with the trusted shell's
    /// environment; efrd's value serves its own check before the call.
    pub shell_path: String,
    /// The limits of the records stream.
    pub limits: RecordLimits,
    /// Where things are.
    pub runtime: RuntimePaths,
    /// The job of `efr-sbx run --job`: a long command with no terminal. `None` for a
    /// call of the hidden shell's wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobPlan>,
    /// The forwarded ports of the conversation's jobs that this call reaches on its own
    /// loopback (version 4): `Some` starts the reach helper in a contained foreground
    /// call with a network namespace of its own. `None` for every other call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<ReachPlan>,
}

/// The reach of a contained foreground call: the reach helper in the call listens on
/// `127.0.0.1` and `::1` of the call's own network namespace for each forwarded port of
/// a job of the same conversation, and sends each connection over the call's reach
/// socket to efrd, which opens it in the job. efrd sends the ports that come or go
/// during the call over the same socket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReachPlan {
    /// The call's reach socket outside, `$CALL/reach.sock`
    /// ([`RuntimePaths::reach_socket`]). The spec check refuses another path.
    pub socket: PathBuf,
    /// The forwarded ports at the prepare: at most [`MAX_REACH_PORTS`], each from
    /// [`MIN_PORT`] and never [`BRIDGE_PORT`], each once.
    pub ports: Vec<u16>,
    /// True in a proxy call: port 3128 is the bridge's.
    pub proxy: bool,
}

impl ReachPlan {
    /// The plan of a call with its reach socket `socket` and the forwarded ports
    /// `ports`. `proxy` is true for a call with a proxy plan.
    pub fn new(socket: PathBuf, ports: Vec<u16>, proxy: bool) -> ReachPlan {
        ReachPlan { socket, ports, proxy }
    }
}

/// The limits of a job (efr's auto spec, section 3.18), and its port forwards.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobPlan {
    /// efrd's time limit of the job, after its clamp. The launcher ends the job itself
    /// [`backstop_secs`](Self::backstop_secs) later, in case efrd is gone.
    pub max_seconds: u64,
    /// The seconds after `max_seconds` at which the launcher ends the job:
    /// [`JOB_BACKSTOP_SECS`] from efrd.
    pub backstop_secs: u64,
    /// `jobs.nice`. The launcher takes the higher of this and its own value, so a job
    /// never runs at a higher priority than the launcher.
    pub nice: u8,
    /// The hard file size limit: [`JOB_FSIZE_BYTES`].
    pub fsize_bytes: u64,
    /// The job's port forwards: `Some` starts the forwarder in a contained job with a
    /// network namespace of its own. `None` for a job with no forwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<ForwardPlan>,
}

impl JobPlan {
    /// The plan of a job with efrd's limit `max_seconds` and the nice value `nice`, with
    /// the fixed backstop and file size limit, and no port forwards.
    pub fn new(max_seconds: u64, nice: u8) -> JobPlan {
        JobPlan {
            max_seconds,
            backstop_secs: JOB_BACKSTOP_SECS,
            nice: nice.min(MAX_NICE),
            fsize_bytes: JOB_FSIZE_BYTES,
            forward: None,
        }
    }

    /// The time after which the launcher ends the job itself.
    pub fn deadline(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.max_seconds.saturating_add(self.backstop_secs))
    }
}

/// The port forwards of a contained job: the forwarder in the job reports each stable
/// loopback listen over the job's forward socket, and efrd forwards the port to the
/// same port of the host's `127.0.0.1`. The timings are fields, so tests set short
/// ones.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardPlan {
    /// The job's forward socket outside, `$CALL/forward.sock`
    /// ([`RuntimePaths::forward_socket`]). The spec check refuses another path, a job
    /// with the whole network and a job with `Grant::NetView`.
    pub socket: PathBuf,
    /// The time between two polls of the job's tcp tables: 500 ms.
    pub poll_ms: u32,
    /// How long a port must listen before it counts: 1000 ms.
    pub stable_ms: u32,
    /// How long a port of the namespace's ephemeral range must listen before it
    /// counts: 10 s.
    pub ephemeral_stable_ms: u32,
    /// The most ports that efr forwards for the job: [`MAX_JOB_FORWARDS`].
    pub max_ports: u8,
    /// True in a proxy job: port 3128 is the bridge's.
    pub proxy: bool,
}

impl ForwardPlan {
    /// The plan of a job with its forward socket `socket`, with the default timings
    /// and limit. `proxy` is true for a job with a proxy plan.
    pub fn new(socket: PathBuf, proxy: bool) -> ForwardPlan {
        ForwardPlan {
            socket,
            poll_ms: 500,
            stable_ms: 1000,
            ephemeral_stable_ms: 10_000,
            max_ports: MAX_JOB_FORWARDS,
            proxy,
        }
    }
}

/// How the launcher runs the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpecLaunch {
    /// bwrap, Landlock and seccomp.
    Contained,
    /// The exit child: the trusted snapshot, no sandbox, a subreaper launcher.
    Unsandboxed,
}

/// Why a directory is writable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WriteRootKind {
    /// The root of the turn's project.
    TurnProject,
    /// A registered project that the line names.
    NamedProject,
    /// The git dir of a worktree or submodule project.
    GitDir,
    /// The common dir of a worktree project.
    GitCommonDir,
    /// `$SCRATCH` of this conversation.
    Scratch,
    /// The private `/tmp` and `/var/tmp`.
    PrivateTmp,
    /// `sandbox.write_roots`.
    UserConfigured,
    /// An approved write grant of this call.
    Grant,
}

/// A writable directory, bound to itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WriteRoot {
    /// The directory, absolute.
    pub path: PathBuf,
    /// Why it is writable.
    pub kind: WriteRootKind,
}

/// A tool cache, mounted as an overlay with a private upper layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheOverlay {
    /// The user's cache directory: the lower layer and the mount point.
    pub target: PathBuf,
    /// The upper layer, below `$SBX/cache/<name>/`.
    pub upper: PathBuf,
    /// The overlay's work dir, next to `upper`.
    pub work: PathBuf,
    /// Paths inside the cache that stay read-only, such as `~/.cargo/bin`.
    pub pins: Vec<PathBuf>,
    /// In the `tmp` mode, keep the private layer of `upper` and `work` across the
    /// conversation's calls, as the `overlay` mode does (`sandbox.keep_downloads`). The
    /// other modes ignore it.
    #[serde(default)]
    pub keep: bool,
}

impl CacheOverlay {
    /// The overlay of the cache `target` with its layers below `sandbox_dir` and the
    /// read-only pins of the spec's cache table (`~/.cargo`: `bin`, `config.toml`,
    /// `config`, `env`; `~/.rustup`: `settings.toml`).
    pub fn new(target: &Path, sandbox_dir: &Path, home: &Path) -> CacheOverlay {
        let layer = sandbox_dir.join("cache").join(layer_name(target));
        let pins: &[&str] = if target == home.join(".cargo") {
            &["bin", "config.toml", "config", "env"]
        } else if target == home.join(".rustup") {
            &["settings.toml"]
        } else {
            &[]
        };
        CacheOverlay {
            target: target.to_path_buf(),
            upper: layer.join("upper"),
            work: layer.join("work"),
            pins: pins.iter().map(|pin| target.join(pin)).collect(),
            keep: false,
        }
    }

    /// True when the cache mounts on its private layer of `upper` and `work` in the
    /// mode `mode`: always in the `overlay` mode, in the `tmp` mode only when
    /// [`keep`](Self::keep) is set, never in the `readonly` mode.
    pub fn keeps_layer(&self, mode: CacheMode) -> bool {
        match mode {
            CacheMode::Overlay => true,
            CacheMode::Tmp => self.keep,
            _ => false,
        }
    }
}

/// A stable directory name for the layers of the cache at `target`: its components
/// joined by `%`, with `%` itself escaped, so two caches never share a layer.
pub fn layer_name(target: &Path) -> String {
    target
        .iter()
        .skip(1)
        .map(|part| part.to_string_lossy().replace('%', "%25"))
        .collect::<Vec<_>>()
        .join("%")
}

/// Why a path is masked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MaskKind {
    /// A secret of the engine (`HOME_SECRETS`, `SYSTEM_SECRETS`,
    /// `permissions.secret_paths`): no grant unmasks it.
    EngineSecret,
    /// A sandbox-only mask (`SANDBOX_MASKS`): a `masked_read` exit unmasks it.
    SandboxMask,
    /// A project `.env` file (`sandbox.mask_globs`).
    ProjectEnv,
    /// `sandbox.mask`.
    User,
    /// efr's own data, state or runtime root; the plan adds these.
    EfrState,
}

/// A path that reads as empty: an empty directory, or `/dev/null` for a file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mask {
    /// The path, absolute. The plan follows its links and masks the real target.
    pub path: PathBuf,
    /// Why.
    pub kind: MaskKind,
}

/// Why a path is a floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FloorKind {
    /// efr's config root and the targets of its links.
    Config,
    /// A shell startup file.
    ShellStartup,
    /// Autostart and service dirs.
    Autostart,
    /// A directory of the hidden shell's `PATH` in a write root.
    PathDir,
    /// A tool config that runs code.
    ToolConfig,
    /// The target of a dotfile link.
    LinkTarget,
    /// efrd, efr, efr-sbx and the zsh assets.
    EfrBinary,
    /// A git config file.
    GitConfig,
    /// A git hooks dir.
    GitHooks,
    /// The `.git` file of a worktree or submodule project.
    GitFile,
    /// An agent or editor config in a write root (`PROTECTED_NAMES`).
    ProtectedName,
    /// `sandbox.protect`.
    User,
    /// `$SCRATCH/jobs`: the job outputs that efrd writes, read-only for every
    /// contained call.
    JobOutput,
    /// A git dir that a job finds at its start, pinned or nested: read-only whole, so a
    /// job that runs for hours cannot plant a hook or a config in it.
    JobGitDir,
}

/// A path that stays read-only even inside a write root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Floor {
    /// The path, absolute. The plan follows its links and binds the real target.
    pub path: PathBuf,
    /// Why.
    pub kind: FloorKind,
}

/// The network of a call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NetworkPlan {
    /// A network namespace with loopback only (phase 1).
    None,
    /// The host's network: an approved `host` or `host_view` exit.
    Open,
    /// The bridge to this call's proxy socket (phase D2): a network namespace with
    /// loopback only, and the bridge on `127.0.0.1:3128` inside.
    Proxy {
        /// The call's proxy socket, outside the sandbox: `$CALL/proxy.sock`
        /// ([`RuntimePaths::proxy_socket`]).
        socket: PathBuf,
    },
}

/// The environment lists of `[sandbox]`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvPlan {
    /// `sandbox.env_deny`: more names removed, `*` patterns allowed.
    pub deny: Vec<String>,
    /// `sandbox.env_keep`: names that the scrub of secret-like names keeps.
    pub keep: Vec<String>,
    /// `sandbox.promote_env`: names that may return to the trusted shell.
    pub promote: Vec<String>,
    /// `sandbox.export_deny`: more names that never return.
    pub export_deny: Vec<String>,
    /// `sandbox.offline_hints`: set the offline variables while there is no network.
    pub offline_hints: bool,
}

/// The limits of the records that a call writes on fd 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecordLimits {
    /// The most bytes of the whole stream.
    pub max_bytes: usize,
    /// The most records.
    pub max_records: usize,
    /// The most bytes of an exported value that may return to the trusted shell.
    pub max_value: usize,
}

impl Default for RecordLimits {
    fn default() -> Self {
        RecordLimits { max_bytes: 256 * 1024, max_records: 4096, max_value: 4096 }
    }
}

/// Where things are, outside and inside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePaths {
    /// `$HOME`.
    pub home: PathBuf,
    /// `$XRT`, the user's runtime dir: an empty tmpfs inside.
    pub user_runtime: PathBuf,
    /// `$R`, efr's runtime root: masked.
    pub runtime: PathBuf,
    /// `$D`, efr's data root: masked; only this conversation's scratch comes back.
    pub data: PathBuf,
    /// `$S`, efr's state root: masked.
    pub state: PathBuf,
    /// `$C`, efr's config root: readable, a floor.
    pub config: PathBuf,
    /// `$SBX`, `$S/sandbox/<conversation>`: private tmp, cache layers, quarantine.
    pub sandbox_dir: PathBuf,
    /// `$R/sbx/<conversation>`: `snapshot.zsh`, `state.zsh`.
    pub shell_dir: PathBuf,
    /// `$CALL`, `$R/sbx/<conversation>/<call>`: spec, line, nonce, apply, result. A
    /// job's call dir is `$R/sbx/<conversation>/jobs/<call>`.
    pub call_dir: PathBuf,
    /// `$SCRATCH` of this conversation.
    pub scratch: PathBuf,
    /// The launcher copy, `$R/bin/efr-sbx`.
    pub launcher: PathBuf,
    /// The child shell's script, `$R/zsh/efr-child.zsh`.
    pub child_script: PathBuf,
    /// The editor stub that `EDITOR` names.
    pub editor: PathBuf,
    /// The zsh program.
    pub zsh: PathBuf,
    /// The bubblewrap program.
    pub bwrap: PathBuf,
    /// The session bus socket, when the user has one.
    pub session_bus: Option<PathBuf>,
    /// The system bus socket.
    pub system_bus: PathBuf,
    /// The desktop sockets that efrd found for the terminal of the prompt, at most one
    /// per desktop. Only a `desktop` grant binds one; none without that grant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktops: Vec<DesktopSocket>,
}

/// The control socket of one desktop, and the value of the variable that names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopSocket {
    /// Which desktop.
    pub desktop: DesktopKind,
    /// The socket, absolute, such as
    /// `/run/user/1000/hypr/<signature>/.socket.sock`.
    pub path: PathBuf,
    /// The value of [`DesktopKind::variable`] for the call, such as the Hyprland
    /// instance signature or `:0`.
    pub value: String,
}

impl RuntimePaths {
    /// The directory inside the sandbox that holds the launcher's files.
    pub fn inside_dir(&self) -> PathBuf {
        self.user_runtime.join("efr-sbx")
    }

    /// The launcher inside the sandbox, which bwrap runs as `efr-sbx inner`.
    pub fn inside_launcher(&self) -> PathBuf {
        self.inside_dir().join("efr-sbx")
    }

    /// The child shell's script inside the sandbox.
    pub fn inside_child_script(&self) -> PathBuf {
        self.inside_dir().join("child.zsh")
    }

    /// The call's proxy socket outside, `$CALL/proxy.sock`, which efrd binds and the
    /// spec of a proxy plan names.
    pub fn proxy_socket(&self) -> PathBuf {
        self.call_dir.join(PROXY_SOCKET_FILE)
    }

    /// `$XRT/efr-proxy.sock`: where the bridge finds the call's proxy socket inside. The
    /// plan binds it read-only, and the child gets no rule to connect to it.
    pub fn inside_proxy_socket(&self) -> PathBuf {
        self.user_runtime.join("efr-proxy.sock")
    }

    /// A job's forward socket outside, `$CALL/forward.sock`, which efrd binds and the
    /// job's [`ForwardPlan`] names.
    pub fn forward_socket(&self) -> PathBuf {
        self.call_dir.join(FORWARD_SOCKET_FILE)
    }

    /// `$XRT/efr-forward.sock`: where the forwarder finds the job's forward socket
    /// inside. The plan binds it read-only, and the child gets no rule to connect to
    /// it.
    pub fn inside_forward_socket(&self) -> PathBuf {
        self.user_runtime.join("efr-forward.sock")
    }

    /// A call's reach socket outside, `$CALL/reach.sock`, which efrd binds and the
    /// call's [`ReachPlan`] names.
    pub fn reach_socket(&self) -> PathBuf {
        self.call_dir.join(REACH_SOCKET_FILE)
    }

    /// `$XRT/efr-reach.sock`: where the reach helper finds the call's reach socket
    /// inside. The plan binds it read-only, and the child gets no rule to connect to
    /// it.
    pub fn inside_reach_socket(&self) -> PathBuf {
        self.user_runtime.join("efr-reach.sock")
    }

    /// The dir of the call's filtered bus sockets, `$CALL/bus/` (0700, made by efrd):
    /// the bus proxy's working dir and the only dir where it may make or remove a file.
    pub fn bus_filter_dir(&self) -> PathBuf {
        self.call_dir.join(BUS_FILTER_DIR)
    }

    /// The call's filtered bus socket outside: `$CALL/bus/system.sock` or
    /// `$CALL/bus/session.sock`, which efrd's bus proxy serves.
    pub fn bus_filter_socket(&self, bus: BusKind) -> PathBuf {
        self.bus_filter_dir().join(bus_filter_file(bus))
    }

    /// Where the child finds `bus` inside: `/run/dbus/system_bus_socket` or `$XRT/bus`,
    /// the places where a D-Bus client looks with no variable. The plan binds the
    /// call's filtered socket there.
    pub fn inside_bus_socket(&self, bus: BusKind) -> PathBuf {
        if bus == BusKind::System {
            PathBuf::from(crate::plan::SYSTEM_BUS_SOCKET)
        } else {
            self.user_runtime.join("bus")
        }
    }

    /// The private tmp's directory outside, `$SBX/tmp`.
    pub fn private_tmp(&self) -> PathBuf {
        self.sandbox_dir.join("tmp")
    }

    /// The quarantine of one call, `$SBX/quarantine/<call>`.
    pub fn quarantine(&self, call: CallId) -> PathBuf {
        self.sandbox_dir.join("quarantine").join(call.to_string())
    }

    /// The state dir of a job, `$SBX/jobs/<call>`: its cache layers and its guard
    /// files, on disk.
    pub fn job_state_dir(&self, call: CallId) -> PathBuf {
        self.sandbox_dir.join(JOB_DIR).join(call.to_string())
    }

    /// `$SCRATCH/jobs`, where efrd writes the job outputs.
    pub fn job_outputs(&self) -> PathBuf {
        self.scratch.join(JOB_DIR)
    }
}

impl SandboxSpec {
    /// The spec as the bytes of `spec.json`.
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec_pretty(self)
            .map_err(|source| SandboxError::Json { what: "spec", source })
    }

    /// Reads `spec.json` and checks it.
    pub fn from_json(bytes: &[u8]) -> Result<SandboxSpec, SandboxError> {
        if bytes.len() > MAX_SPEC_BYTES {
            return Err(SandboxError::TooLarge {
                what: "spec",
                len: bytes.len(),
                max: MAX_SPEC_BYTES,
            });
        }
        let spec: SandboxSpec = serde_json::from_slice(bytes)
            .map_err(|source| SandboxError::Json { what: "spec", source })?;
        spec.check()?;
        Ok(spec)
    }

    /// Checks the version, that every path is absolute and in normal form, that the
    /// call and shell dirs name this call and conversation, that the call dir lies in
    /// the shell dir (in its `jobs` dir for a job) and nowhere else, the limits of a
    /// job, that a proxy plan names the call's own socket and comes with no grant of
    /// the whole network, that a filtered bus comes with no grant of the same whole
    /// bus, that a job's port forwards fit the job, and that a reach fits the call
    /// ([`check_reach`]).
    pub fn check(&self) -> Result<(), SandboxError> {
        if self.version != SPEC_VERSION {
            return Err(SandboxError::SpecVersion { found: self.version, expected: SPEC_VERSION });
        }
        let runtime = &self.runtime;
        let mut paths: Vec<(&'static str, &Path)> = vec![
            ("runtime.home", &runtime.home),
            ("runtime.user_runtime", &runtime.user_runtime),
            ("runtime.runtime", &runtime.runtime),
            ("runtime.data", &runtime.data),
            ("runtime.state", &runtime.state),
            ("runtime.config", &runtime.config),
            ("runtime.sandbox_dir", &runtime.sandbox_dir),
            ("runtime.shell_dir", &runtime.shell_dir),
            ("runtime.call_dir", &runtime.call_dir),
            ("runtime.scratch", &runtime.scratch),
            ("runtime.launcher", &runtime.launcher),
            ("runtime.child_script", &runtime.child_script),
            ("runtime.editor", &runtime.editor),
            ("runtime.zsh", &runtime.zsh),
            ("runtime.bwrap", &runtime.bwrap),
            ("runtime.system_bus", &runtime.system_bus),
        ];
        if let Some(bus) = &runtime.session_bus {
            paths.push(("runtime.session_bus", bus));
        }
        paths.extend(
            runtime.desktops.iter().map(|desktop| ("runtime.desktops", desktop.path.as_path())),
        );
        paths.extend(self.write_roots.iter().map(|root| ("write_roots", root.path.as_path())));
        for cache in &self.caches {
            paths.extend([
                ("caches.target", cache.target.as_path()),
                ("caches.upper", cache.upper.as_path()),
                ("caches.work", cache.work.as_path()),
            ]);
            paths.extend(cache.pins.iter().map(|pin| ("caches.pins", pin.as_path())));
        }
        paths.extend(self.masks.iter().map(|mask| ("masks", mask.path.as_path())));
        paths.extend(self.floors.iter().map(|floor| ("floors", floor.path.as_path())));
        paths.extend(self.git_dirs.iter().map(|dir| ("git_dirs", dir.as_path())));
        paths.extend(self.guard_roots.iter().map(|root| ("guard_roots", root.as_path())));
        paths.extend(
            self.grants.iter().filter_map(|grant| grant.path()).map(|path| ("grants", path)),
        );
        if let NetworkPlan::Proxy { socket } = &self.network {
            paths.push(("network.socket", socket));
        }
        if let Some(forward) = self.job.as_ref().and_then(|job| job.forward.as_ref()) {
            paths.push(("job.forward.socket", &forward.socket));
        }
        if let Some(reach) = &self.reach {
            paths.push(("reach.socket", &reach.socket));
        }
        for (field, path) in paths {
            if !is_normal(path) {
                return Err(SandboxError::SpecPath { field, path: path.to_path_buf() });
            }
        }
        let names = |path: &Path, id: String| path.file_name().is_some_and(|name| *name == *id);
        if !names(&runtime.call_dir, self.call.to_string()) {
            return Err(SandboxError::SpecIds {
                field: "runtime.call_dir",
                path: runtime.call_dir.clone(),
            });
        }
        if !names(&runtime.shell_dir, self.conversation.to_string()) {
            return Err(SandboxError::SpecIds {
                field: "runtime.shell_dir",
                path: runtime.shell_dir.clone(),
            });
        }
        let parent = match self.job {
            Some(_) => runtime.shell_dir.join(JOB_DIR),
            None => runtime.shell_dir.clone(),
        };
        if runtime.call_dir.parent() != Some(parent.as_path()) {
            return Err(SandboxError::SpecIds {
                field: "runtime.call_dir",
                path: runtime.call_dir.clone(),
            });
        }
        if let Some(job) = &self.job
            && (job.max_seconds == 0 || job.nice > MAX_NICE || job.fsize_bytes == 0)
        {
            return Err(SandboxError::SpecJob);
        }
        // NOTE: a filtered bus beside the whole bus of the same bus would bind two
        // sockets at one place. efrd's `view_fits` drops the view first; the launcher
        // checks it again, and no plan step skips it.
        for grant in &self.grants {
            if let Grant::BusFilter { bus } = grant
                && self.grants.contains(&Grant::Bus { bus: *bus })
            {
                return Err(SandboxError::BusFilterWithBus { bus: *bus });
            }
        }
        if let NetworkPlan::Proxy { socket } = &self.network {
            // NOTE: the launcher's own check, not only efrd's: with the whole network the
            // plan keeps the host's namespace, and the bridge would listen on the host's
            // 127.0.0.1:3128.
            if self.grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork)) {
                return Err(SandboxError::ProxyWithOpenNetwork);
            }
            if *socket != runtime.proxy_socket() {
                return Err(SandboxError::ProxySocket { path: socket.clone() });
            }
        }
        if let Some(forward) = self.job.as_ref().and_then(|job| job.forward.as_ref()) {
            self.check_forward(forward)?;
        }
        check_reach(self)?;
        Ok(())
    }

    /// The checks of a job's port forwards: only a contained job with a network
    /// namespace of its own, the job's own socket, the proxy flag of its network, and
    /// timings and a limit that a forwarder can keep.
    fn check_forward(&self, forward: &ForwardPlan) -> Result<(), SandboxError> {
        // NOTE: a job that keeps the host's network namespace listens on the host's
        // loopback already; a forwarder there would serve the host's own ports.
        let host_network = matches!(self.network, NetworkPlan::Open)
            || self.grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork | Grant::NetView));
        if host_network {
            return Err(SandboxError::ForwardWithHostNetwork);
        }
        if forward.socket != self.runtime.forward_socket() {
            return Err(SandboxError::ForwardSocket { path: forward.socket.clone() });
        }
        let proxy = matches!(self.network, NetworkPlan::Proxy { .. });
        if self.launch != SpecLaunch::Contained
            || forward.proxy != proxy
            || forward.poll_ms == 0
            || forward.max_ports == 0
            || forward.max_ports > MAX_JOB_FORWARDS
        {
            return Err(SandboxError::ForwardPlan);
        }
        Ok(())
    }
}

/// The checks of a call's reach: only a contained foreground call with a network
/// namespace of its own, the call's own socket, the proxy flag of its network, and the
/// ports that efr may forward. Refuses a reach plan of a job, of the exit child, of
/// `NetworkPlan::Open`, with `Grant::OpenNetwork` or `Grant::NetView`, with another
/// socket path, with more than [`MAX_REACH_PORTS`] ports, or with a port below
/// [`MIN_PORT`], [`BRIDGE_PORT`] or a port twice. A spec with no reach passes.
pub fn check_reach(spec: &SandboxSpec) -> Result<(), SandboxError> {
    let Some(reach) = &spec.reach else { return Ok(()) };
    // NOTE: a call that keeps the host's network namespace reaches efrd's host
    // listeners already; a helper there would bind the host's own loopback.
    let host_network = matches!(spec.network, NetworkPlan::Open)
        || spec.grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork | Grant::NetView));
    if host_network {
        return Err(SandboxError::ReachWithHostNetwork);
    }
    if reach.socket != spec.runtime.reach_socket() {
        return Err(SandboxError::ReachSocket { path: reach.socket.clone() });
    }
    if spec.job.is_some() {
        return Err(SandboxError::ReachPlan { problem: "a reach of a job" });
    }
    if spec.launch != SpecLaunch::Contained {
        return Err(SandboxError::ReachPlan { problem: "a reach of the exit child" });
    }
    if reach.proxy != matches!(spec.network, NetworkPlan::Proxy { .. }) {
        return Err(SandboxError::ReachPlan { problem: "a proxy flag of another network" });
    }
    if reach.ports.len() > MAX_REACH_PORTS {
        return Err(SandboxError::ReachPlan { problem: "more than 32 ports" });
    }
    for (index, port) in reach.ports.iter().enumerate() {
        if *port < MIN_PORT {
            return Err(SandboxError::ReachPlan { problem: "a port below 1024" });
        }
        if *port == BRIDGE_PORT {
            return Err(SandboxError::ReachPlan { problem: "the port of the proxy bridge" });
        }
        if reach.ports[..index].contains(port) {
            return Err(SandboxError::ReachPlan { problem: "a port twice" });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
