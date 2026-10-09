//! `models.list`: the models that a prompt may name, with their reasoning efforts.

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The params of `models.list`. It takes none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelsList {}

/// The result of `models.list`: the daemon's effective model list, which a prompt's
/// `settings.model` must name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelsListResult {
    /// Every model: the ones of the provider's catalog first, best priority first, then
    /// the ones that the config adds.
    pub models: Vec<ModelInfo>,
    /// Where the provider's catalog came from. Absent when the daemon does not report
    /// it, as before the catalog came from the backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogStatus>,
}

/// Where the daemon's model catalog came from, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogStatus {
    /// The provider whose catalog it is, such as `openai-subscription` or
    /// `anthropic-api`. Absent when the daemon does not report it, as before a daemon
    /// could run more than one kind of provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Where the list came from.
    pub origin: CatalogOrigin,
    /// When the backend last sent the list or said that it did not change. Absent for
    /// the list built into efr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<Timestamp>,
}

/// Where a model catalog came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CatalogOrigin {
    /// The provider's backend sent it to this daemon, or said that it did not change.
    Backend,
    /// The cache file of an earlier fetch. The daemon starts with it, such as while it
    /// is offline, until the backend answers.
    Cache,
    /// The list built into efr: no fetch worked yet and no cache is on disk, or the
    /// provider has no catalog with windows (the API key backend).
    Builtin,
    /// No list yet: no fetch worked and no cache is on disk, and the provider has no
    /// list built into efr (`anthropic-api`). The daemon fetches the list before the
    /// next prompt.
    Missing,
}

/// One model of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelInfo {
    /// The model id, such as `gpt-5.5`.
    pub id: String,
    /// The reasoning efforts that the model takes, such as `low`, `medium` and `high`.
    /// A prompt's `settings.effort` must be one of them. Empty for a model whose
    /// efforts efr does not know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    /// The model's own default effort, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// True for the model that a turn uses when the prompt names none: the config's
    /// `[model] name`, else the daemon's default. At most one model has it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
    /// Where the model comes from.
    pub source: ModelSource,
    /// The context window in tokens that efrd uses for the model: the window of the
    /// model's entry in `[openai] models`, cut down to `max_context_window`, else the
    /// catalog's window. Absent when efr does not know it; efrd then counts with a
    /// default window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// The largest window in tokens that an entry in `[openai] models` can set for the
    /// model, from the catalog. Absent when the catalog does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_window: Option<u64>,
    /// True when the backend prefers that a client reach the model over a WebSocket.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prefer_websockets: bool,
}

/// Where a model of the list comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelSource {
    /// The provider's model catalog: from the backend, its cache or the list built into
    /// efr. [`ModelsListResult::catalog`] says which.
    Builtin,
    /// The `[openai] models` list of the config file.
    Config,
}

/// The longest reasoning effort, in bytes, that a model whose efforts are not known
/// takes.
pub const EFFORT_MAX_LEN: usize = 32;

impl ModelInfo {
    /// True when the model takes `effort`: one of its [`efforts`](Self::efforts), or,
    /// when they are not known, any [effort word](is_effort_word). The daemon, the CLI
    /// and the config all decide with it, so a value that one of them keeps never fails
    /// in another.
    pub fn takes_effort(&self, effort: &str) -> bool {
        if self.efforts.is_empty() {
            is_effort_word(effort)
        } else {
            self.efforts.iter().any(|known| known == effort)
        }
    }
}

/// True for 1 to [`EFFORT_MAX_LEN`] bytes of lowercase ASCII letters, digits, `-` and
/// `_`: the form of every effort a backend names, so a new effort needs no change of
/// efr. It is what a model whose efforts are not known, or no model, takes.
pub fn is_effort_word(effort: &str) -> bool {
    (1..=EFFORT_MAX_LEN).contains(&effort.len())
        && effort.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

#[cfg(test)]
mod tests;
