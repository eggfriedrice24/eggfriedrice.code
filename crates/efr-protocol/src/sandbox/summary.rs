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
    /// The programs of an approved exit's descendants that efr could not end, such as a
    /// process that `sudo` left running as root. They may still hold the hidden
    /// shell's terminal, so efr starts a new shell for the next call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub survivors: Vec<String>,
    /// Connections that the proxy refused (phase D), at most
    /// [`MAX_BLOCKED`](Self::MAX_BLOCKED): one entry for each host, port and reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<Blocked>,
    /// The refusals that `blocked` leaves out (phase D2).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub blocked_more: u32,
    /// True when the call ran with a proxy plan (phase D2). The CLI's trace reads it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proxied: bool,
    /// Git settings and other files that run code, which the call changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surface_changes: Vec<SurfaceChange>,
    /// Why the sandbox could not start, such as bubblewrap's setup message. The command
    /// did not run, and efr checks the sandbox again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_error: Option<String>,
    /// The paths in the tool caches that the call deleted in its private layer, at most
    /// [`MAX_CACHE_DELETES`](Self::MAX_CACHE_DELETES) and
    /// [`MAX_CACHE_DELETE_BYTES`](Self::MAX_CACHE_DELETE_BYTES). efr asks whether to
    /// delete them in the real cache too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_deletes: Vec<PathBuf>,
    /// The cache deletes that `cache_deletes` leaves out: over its limits, a name that
    /// is not UTF-8, or a whole directory hidden at once.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_deletes_more: u32,
}

impl SandboxSummary {
    /// The most entries that [`blocked`](Self::blocked) lists.
    pub const MAX_BLOCKED: usize = 32;
    /// The most paths that [`cache_deletes`](Self::cache_deletes) lists.
    pub const MAX_CACHE_DELETES: usize = 4096;
    /// The most bytes of paths that [`cache_deletes`](Self::cache_deletes) lists.
    pub const MAX_CACHE_DELETE_BYTES: usize = 256 * 1024;
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

fn one() -> u32 {
    1
}

fn is_one(count: &u32) -> bool {
    *count == 1
}

/// A connection that the proxy refused (phase D).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Blocked {
    /// The host, or `<invalid>` when the request named no valid host.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Why.
    pub reason: BlockReason,
    /// How many connections of the call the proxy refused for this host, port and
    /// reason. 1 when absent.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub count: u32,
}

/// Why the proxy refused a connection (phase D).
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
    /// The host is an IP address that no `[[network.hosts]]` entry lists.
    IpLiteral,
    /// The first bytes of a tunnel were no TLS ClientHello with a server name, or the
    /// hello asked for Encrypted Client Hello.
    NotTls,
    /// The request head was malformed: control characters, a body on GET, a Host
    /// header that differs, or not the absolute form.
    BadRequest,
    /// The call had too many open tunnels.
    Limit,
    /// A reason that this build does not know.
    #[serde(other)]
    Unknown,
}

impl BlockReason {
    /// Every reason, in the order of the enum.
    pub const ALL: [BlockReason; 10] = [
        BlockReason::NotAllowed,
        BlockReason::PrivateAddress,
        BlockReason::SniMismatch,
        BlockReason::UploadLimit,
        BlockReason::Method,
        BlockReason::IpLiteral,
        BlockReason::NotTls,
        BlockReason::BadRequest,
        BlockReason::Limit,
        BlockReason::Unknown,
    ];
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

impl SurfaceChange {
    /// What the change does, for a person: the code key it sets, else its rule in
    /// words, such as `a new config.worktree`. A rule that this build does not know
    /// shows its name with spaces.
    pub fn what(&self) -> String {
        if let Some(key) = &self.key {
            return key.clone();
        }
        let words = match self.rule.as_str() {
            "commondir_in_main_git_dir" => "a commondir that points git at another config",
            "config_worktree_appeared" | "worktree_config_worktree_appeared" => {
                "a new config.worktree"
            }
            "worktree_commondir_elsewhere" => "a commondir that points away from its main git dir",
            "config_code_key" => "a key that runs a program",
            "config_unreadable" => "a config that git cannot read",
            "commondir_outside_repo" => "a commondir that points outside the repository",
            "hook_planted" => "a new hook",
            "module_code_key" => "a submodule key that runs a program",
            "module_hook" => "a new submodule hook",
            "alternates_changed" => "git objects from another place",
            "protected_name_created" => "a new agent or editor config",
            other => return other.replace('_', " "),
        };
        words.to_owned()
    }
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
