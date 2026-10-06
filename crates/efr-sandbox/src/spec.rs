//! [`SandboxSpec`]: everything one call's launch needs, as plain paths and names.
//!
//! efrd builds the spec from the turn, the settings, the registry, its own roots and
//! the engine's secret tables, and writes it to `$CALL/spec.json` (0600, in a 0700
//! dir of the masked runtime root). `efr-sbx run` reads it, checks it and builds the
//! [`MountPlan`](crate::MountPlan). This crate never reads a config file or a secret
//! table itself, so the engine and the sandbox cannot disagree about a secret.

use std::path::{Path, PathBuf};

use efr_protocol::{CacheMode, CallId, ConversationId, Grant};
use serde::{Deserialize, Serialize};

use crate::SandboxError;
use crate::paths::is_normal;

/// The version of the spec that this build writes and reads.
pub const SPEC_VERSION: u32 = 1;

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
    /// The bridge to this call's proxy socket (phase 2).
    Proxy {
        /// The call's proxy socket, outside the sandbox.
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
    /// `$CALL`, `$R/sbx/<conversation>/<call>`: spec, line, nonce, apply, result.
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

    /// The private tmp's directory outside, `$SBX/tmp`.
    pub fn private_tmp(&self) -> PathBuf {
        self.sandbox_dir.join("tmp")
    }

    /// The quarantine of one call, `$SBX/quarantine/<call>`.
    pub fn quarantine(&self, call: CallId) -> PathBuf {
        self.sandbox_dir.join("quarantine").join(call.to_string())
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

    /// Checks the version, that every path is absolute and in normal form, and that the
    /// call and shell dirs name this call and conversation.
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
        Ok(())
    }
}

#[cfg(test)]
mod tests;
