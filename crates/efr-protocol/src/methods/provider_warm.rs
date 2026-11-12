//! `provider.warm`: make the connection of a terminal's next prompt ready while the user
//! types it, for the zsh plugin.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The params of `provider.warm`. The daemon picks the provider, the model and the
/// conversation that the next prompt of the terminal would use, and makes that
/// provider's connection ready: it refreshes a token that ends soon and opens, or
/// checks, the connection. It never sends a model request, so it never costs a token,
/// and the params carry no text of the prompt. A warm-up that does not work is silent;
/// the prompt does the work as before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderWarm {
    /// The terminal of the next prompt. Absent: the terminal that the connection's
    /// `hello` named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The working directory of the next prompt, which picks the terminal's
    /// conversation of that scope. Absent: no conversation, only the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pwd: Option<PathBuf>,
    /// The model that the terminal's turn settings ask for. Absent: the model of the
    /// conversation's pick, else the config's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The provider or account of `model`, as `TurnSettings::provider` names it, such
    /// as `work`. Only with `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// True when the next prompt starts a new conversation (`,new`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_conversation: bool,
}

/// The result of `provider.warm`. It comes before the warm-up ends, which runs on in
/// the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderWarmResult {
    /// The provider that the warm-up is for, such as `openai-subscription`. This
    /// daemon always sends it; a client must also accept an answer without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}
