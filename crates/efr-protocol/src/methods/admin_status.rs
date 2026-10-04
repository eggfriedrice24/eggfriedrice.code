//! `admin.status`: the daemon's health, for `efr status`.

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::DaemonId;

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
}

/// The state of one model provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderStatus {
    /// The provider, such as `openai`.
    pub provider: String,
    /// True when credentials are stored.
    pub logged_in: bool,
    /// When the current access token expires, for a provider with refreshable tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}
