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
