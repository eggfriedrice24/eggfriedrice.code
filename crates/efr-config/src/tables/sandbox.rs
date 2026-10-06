//! `[sandbox]`: the kernel sandbox of the `auto` mode (phase 1 keys).

use std::path::PathBuf;

use efr_protocol::CacheMode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The tool caches that the sandbox mounts as private overlays when they exist.
pub const DEFAULT_CACHES: &[&str] = &[
    "~/.cargo",
    "~/.rustup",
    "~/.cache",
    "~/go/pkg/mod",
    "~/.npm",
    "~/.bun/install/cache",
    "~/.local/share/pnpm/store",
    "~/.m2/repository",
    "~/.gradle/caches",
];

/// The names masked inside write roots; a leading `!` keeps a name readable.
pub const DEFAULT_MASK_GLOBS: &[&str] =
    &[".env", ".env.*", "!.env.example", "!.env.sample", "!.env.template"];

/// The exported names that return from a sandboxed call to the hidden shell.
pub const DEFAULT_PROMOTE_ENV: &[&str] = &[
    "RUST_LOG",
    "RUST_BACKTRACE",
    "NODE_ENV",
    "DEBUG",
    "CI",
    "NO_COLOR",
    "FORCE_COLOR",
    "TZ",
    "LANG",
    "LC_*",
];

/// The folders that a sync service copies off the machine.
pub const DEFAULT_SYNCED_DIRS: &[&str] =
    &["~/Dropbox", "~/Nextcloud", "~/Sync", "~/OneDrive", "~/MEGA"];

/// The files that run code later outside the sandbox, for the turn-end report.
pub const DEFAULT_SURFACE_FILES: &[&str] = &[
    "Makefile",
    "build.rs",
    "package.json",
    ".envrc",
    "rust-toolchain.toml",
    "justfile",
    "*.mk",
    "CMakeLists.txt",
    "setup.py",
    "pyproject.toml",
    "conftest.py",
    ".vscode/tasks.json",
    ".nvim.lua",
    ".exrc",
    ".mise.toml",
    ".tool-versions",
    "flake.nix",
    ".pre-commit-config.yaml",
    ".cargo/config.toml",
    ".cargo/config",
    ".npmrc",
    ".yarnrc.yml",
    ".pnpmfile.cjs",
];

/// The directories whose deletion is not destructive: builds make them again.
pub const DEFAULT_REBUILDABLE: &[&str] = &["target", "node_modules", "dist", "build", ".venv"];

/// Which registered projects a call of the `auto` mode may write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum WriteProjects {
    /// The turn's project only.
    Turn,
    /// The turn's project and the registered projects that the command names.
    #[default]
    Named,
    /// Every registered project.
    All,
}

impl WriteProjects {
    /// The name in the file, such as `named`.
    pub const fn as_str(self) -> &'static str {
        match self {
            WriteProjects::Turn => "turn",
            WriteProjects::Named => "named",
            WriteProjects::All => "all",
        }
    }
}

/// `[sandbox]`: the kernel sandbox of the `auto` mode. Every key applies from the next
/// call or turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SandboxSettings {
    /// `false` makes the `auto` mode unavailable: its turns run as `cautious`.
    pub enabled: bool,
    /// The bubblewrap program, an absolute path. Unset: the first `bwrap` on the
    /// daemon's `PATH`. A change runs the probe again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bwrap: Option<PathBuf>,
    /// Which registered projects a call may write: `turn` (the turn's project), `named`
    /// (also the registered projects that the command names) or `all`.
    pub write_projects: WriteProjects,
    /// More write roots for every call: absolute, or below the home directory as
    /// `~/...`. Floors still apply inside them. The home directory itself is refused.
    pub write_roots: Vec<PathBuf>,
    /// The tool caches that a call reads and writes through a private overlay:
    /// absolute, or `~/...`. A cache that does not exist is skipped.
    pub caches: Vec<PathBuf>,
    /// How the caches are mounted: `overlay` (a private upper layer per conversation),
    /// `tmp` (writes vanish after each call) or `readonly`.
    pub cache_mode: CacheMode,
    /// Days without a call after which a conversation's cache layers are deleted, from 1
    /// to 3650.
    pub cache_days: u32,
    /// The size of all cache layers together, in GiB, from 1 to 10000; the oldest
    /// conversation's layers go first.
    pub cache_max_gib: u32,
    /// More paths that a call reads as empty: absolute, or `~/...`. In `auto`, a
    /// `read_file` of them asks.
    pub mask: Vec<PathBuf>,
    /// Names masked inside each write root, to depth 3: `*` matches any text, and a
    /// leading `!` keeps a name readable.
    pub mask_globs: Vec<String>,
    /// More paths that stay read-only even inside a write root: absolute, or `~/...`.
    pub protect: Vec<PathBuf>,
    /// More environment variables removed from every call: names, or patterns with
    /// `*`.
    pub env_deny: Vec<String>,
    /// Variables that the scrub of secret-like names keeps: names, or patterns with
    /// `*`.
    pub env_keep: Vec<String>,
    /// The exported variables that return from a call to the hidden shell: names, or
    /// patterns with `*`. A name of the built-in never list never returns.
    pub promote_env: Vec<String>,
    /// More names that never return to the hidden shell: names, or patterns with `*`.
    pub export_deny: Vec<String>,
    /// Folders that a sync service copies off the machine: absolute, or `~/...`. A write
    /// to them is an exit that only you can approve.
    pub synced_dirs: Vec<PathBuf>,
    /// The files that run code later outside the sandbox: names or paths relative to a
    /// write root, with `*`. The end of an `auto` turn lists the ones it changed.
    pub surface_files: Vec<String>,
    /// Tell package managers to work offline while the sandbox has no network
    /// (`CARGO_NET_OFFLINE`, `npm_config_offline`, `UV_OFFLINE`, `GOPROXY=off`).
    pub offline_hints: bool,
    /// Directory names whose deletion is not destructive, because builds make them
    /// again.
    pub rebuildable: Vec<String>,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        let paths = |list: &[&str]| list.iter().map(PathBuf::from).collect();
        let names = |list: &[&str]| list.iter().map(|name| (*name).to_owned()).collect();
        SandboxSettings {
            enabled: true,
            bwrap: None,
            write_projects: WriteProjects::Named,
            write_roots: Vec::new(),
            caches: paths(DEFAULT_CACHES),
            // NOTE: tmp, not overlay: overlays failed the phase 1 gate of the auto spec on
            // the reference machine (p95 above 10 ms, EBUSY setup failures in 1000 calls).
            cache_mode: CacheMode::Tmp,
            cache_days: 14,
            cache_max_gib: 20,
            mask: Vec::new(),
            mask_globs: names(DEFAULT_MASK_GLOBS),
            protect: Vec::new(),
            env_deny: Vec::new(),
            env_keep: Vec::new(),
            promote_env: names(DEFAULT_PROMOTE_ENV),
            export_deny: Vec::new(),
            synced_dirs: paths(DEFAULT_SYNCED_DIRS),
            surface_files: names(DEFAULT_SURFACE_FILES),
            offline_hints: true,
            rebuildable: names(DEFAULT_REBUILDABLE),
        }
    }
}
