//! `admin.login_api_key`: store an API key for a provider, for `efr login openai-api`
//! and `efr login anthropic`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SecretText;

/// The params of `admin.login_api_key`, a unary admin method (Unix socket only).
///
/// The daemon refuses a key that is empty or holds whitespace or a character outside
/// ASCII, checks the key with one request to the provider that runs no model unless
/// `check` is false, stores it as the provider's credential, records
/// `login_completed` and answers. The key never reaches an event, a log line or the
/// text of an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminLoginApiKey {
    /// The provider of the key: `openai-api` or `anthropic-api`.
    pub provider: String,
    /// The key, as the provider issued it.
    pub key: SecretText,
    /// True to check the key with the provider before the daemon stores it. True when
    /// absent.
    #[serde(default = "check_by_default")]
    pub check: bool,
}

const fn check_by_default() -> bool {
    true
}

/// The result of `admin.login_api_key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminLoginApiKeyResult {
    /// The provider that is now logged in.
    pub provider: String,
    /// What a client may show of the stored key: its known prefix and its last four
    /// characters, such as `sk-ant-...a1b2`. Never the key.
    pub key_hint: String,
    /// True when the provider accepted the key; false when the check was skipped.
    pub checked: bool,
    /// True when the provider is the one of new conversations, which use the key at
    /// once. Another provider needs `[model] provider` and a restart of the daemon.
    pub active: bool,
}
