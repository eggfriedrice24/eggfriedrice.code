//! `admin.status`: the daemon's health, for `efr status`, and where it keeps its files
//! and reads its config, for `efr paths` and `efr config show`.

use std::path::PathBuf;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CatalogStatus, ConfigFileError, DaemonId, SandboxPaths, SandboxStatus};

/// The params of `admin.status`, an admin method (Unix socket only). It takes none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminStatus {}

/// The result of `admin.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminStatusResult {
    /// The daemon's identity.
    pub daemon_id: DaemonId,
    /// The daemon's build version.
    pub version: String,
    /// The daemon's [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION).
    pub protocol: u32,
    /// The daemon's process id.
    pub pid: u32,
    /// When the daemon started.
    pub started_at: Timestamp,
    /// The screen backend in use: `vt100` or `ghostty`.
    pub screen_backend: String,
    /// Conversations with a live actor.
    pub conversations: u32,
    /// Hidden shells that are running.
    pub shells: u32,
    /// The configured model providers.
    pub providers: Vec<ProviderStatus>,
    /// Where the model catalog of the active provider came from. Absent when the
    /// daemon does not report it, as before the catalog came from the backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogStatus>,
    /// The daemon's four root directories and where each came from, so a client can
    /// warn when its own differ. Absent when the daemon does not report them, as before
    /// `efr paths`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<DaemonRoots>,
    /// The config file that the daemon reads and the state of its last reload. Absent
    /// when the daemon does not report it, as before live reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigStatus>,
    /// The sandbox of the `auto` mode, as the last probe found it. Absent when the
    /// daemon does not report it, as before the sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxStatus>,
    /// Where the sandbox keeps its launcher and its files, for `efr paths`. Absent when
    /// the daemon does not report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_paths: Option<SandboxPaths>,
}

/// The state of one model provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderStatus {
    /// The provider, such as `openai-subscription`.
    pub provider: String,
    /// True when credentials are stored.
    pub logged_in: bool,
    /// When the current access token expires, for a provider with refreshable tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// True for the provider of new conversations, `[model] provider` of the running
    /// daemon. False when absent, as from an earlier daemon.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
    /// How the provider is logged in. Absent when it is not, or from an earlier daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<LoginKind>,
    /// For an API key: its known prefix and its last four characters, such as
    /// `sk-ant-...a1b2`. Never the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
}

/// How a provider is logged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoginKind {
    /// A subscription login in a browser, with tokens that the daemon refreshes.
    Subscription,
    /// An API key, which cannot refresh.
    ApiKey,
}

/// The daemon's four root directories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonRoots {
    /// The config root, which holds `config.toml` and `projects.toml`.
    pub config: RootDir,
    /// The data root: the database, recordings, scratch directories and secrets.
    pub data: RootDir,
    /// The state root: the logs.
    pub state: RootDir,
    /// The runtime root: the socket and `daemon.json`.
    pub runtime: RootDir,
}

/// One root directory and where its path came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RootDir {
    /// The directory.
    pub path: PathBuf,
    /// Where the path came from.
    pub source: RootSource,
}

/// Where a root directory's path came from, in the order the daemon looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RootSource {
    /// The root's own variable, such as `EFR_DATA_DIR`.
    DirVariable,
    /// A directory below `EFR_HOME`, such as `$EFR_HOME/data`.
    EfrHome,
    /// The XDG base directory, such as `$XDG_DATA_HOME/efr`.
    Xdg,
    /// `/run/user/<uid>/efr`, for the runtime root when `XDG_RUNTIME_DIR` is unset.
    RunUser,
}

/// The config file that the daemon reads, and the state of its last reload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigStatus {
    /// `config.toml` in the daemon's config root.
    pub path: PathBuf,
    /// True when `path` names a file, directly or through a symlink. A missing file is
    /// an empty config: every value is its default.
    pub exists: bool,
    /// The file that `path` points to when it is a symlink, as an absolute path, also
    /// when that file is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<PathBuf>,
    /// What was wrong with the file at the last reload, which kept the old settings.
    /// Absent once the file loads again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload_error: Option<ConfigFileError>,
    /// The dotted keys whose new values in the file apply only after efrd restarts,
    /// such as `screen`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restart_needed: Vec<String>,
}
