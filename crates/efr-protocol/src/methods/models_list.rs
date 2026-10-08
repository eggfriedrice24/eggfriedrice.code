//! `models.list`: the models that a prompt may name, with their reasoning efforts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The params of `models.list`. It takes none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelsList {}

/// The result of `models.list`: the daemon's effective model list, which a prompt's
/// `settings.model` must name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelsListResult {
    /// Every model, the built-in ones first, then the ones that the config adds.
    pub models: Vec<ModelInfo>,
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
    /// The model's context window in tokens: from efr's built-in list, or from the
    /// model's entry in `[openai] models`. Absent when efr does not know it; efrd then
    /// counts with a default window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// Where a model of the list comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelSource {
    /// The daemon's built-in list for its provider.
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
