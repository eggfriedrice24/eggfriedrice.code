//! `admin.logout`: forget the credentials of a provider, for `efr logout`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The params of `admin.logout`, a unary admin method (Unix socket only).
///
/// The daemon deletes the provider's stored credentials, records `logout_completed`,
/// and the provider's next request fails as not logged in. The provider keeps an API
/// key valid: only its owner can revoke it at the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminLogout {
    /// The provider: `openai-subscription`, `openai-api` or `anthropic-api`.
    pub provider: String,
}

/// The result of `admin.logout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminLogoutResult {
    /// The provider.
    pub provider: String,
    /// True when credentials were stored and are now deleted; false when there were
    /// none.
    pub logged_out: bool,
    /// True when the provider is the one of new conversations (`[model] provider`):
    /// its turns fail until a new login. False when absent, as from an earlier daemon.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
}
