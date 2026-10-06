//! What a call through the launcher reports back: names only, never values or file
//! contents.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a contained or unsandboxed call did to the state around it. Names only: no
/// value of a variable and no file content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxSummary {
    /// True when the call ran in the sandbox; false for the exit child.
    pub confined: bool,
    /// True when the hidden shell's working directory changed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cwd_changed: bool,
    /// Exported names that reached the hidden shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub promoted: Vec<String>,
    /// Exported names that stay in the sandbox's state only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_out: Vec<String>,
    /// Exported names that were dropped everywhere, such as `LD_PRELOAD`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    /// The programs of background jobs that stopped when the call ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background_stopped: Vec<String>,
    /// Connections that the proxy refused (phase 2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<Blocked>,
    /// Git settings and other files that run code, which the call changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surface_changes: Vec<SurfaceChange>,
}

/// A connection that the proxy refused (phase 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Blocked {
    /// The host.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Why.
    pub reason: BlockReason,
}

/// Why the proxy refused a connection (phase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BlockReason {
    /// The host is not on the allow list.
    NotAllowed,
    /// The host resolved to a private or local address.
    PrivateAddress,
    /// The TLS server name differs from the requested host.
    SniMismatch,
    /// The call sent more than its upload budget.
    UploadLimit,
    /// The request method is not allowed.
    Method,
}

/// One change to a git setting or another file that runs code, found by the surface
/// guard after a call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SurfaceChange {
    /// The file or directory that changed.
    pub path: PathBuf,
    /// The rule it broke, such as `commondir_in_main_git_dir`.
    pub rule: String,
    /// The code key it sets, such as `core.fsmonitor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// True when the launcher moved it to quarantine, where nothing reads it.
    pub quarantined: bool,
}

/// A file that the turn changed and that runs code later outside the sandbox, for the
/// turn-end report.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ReportedFile {
    /// The file, relative to its write root when it lies in one.
    pub path: PathBuf,
    /// What runs, such as `scripts` or `build.rustc-wrapper`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
