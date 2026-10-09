//! `[anthropic]`: the Anthropic provider, `anthropic-api`. The rules of the provider
//! are in the README of `efr-provider-anthropic`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ModelEntry;

/// How long Anthropic's prompt cache keeps the entries that a request writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub enum CacheTtlChoice {
    /// efr's choice, today one hour on every cache entry: an entry for one hour cannot
    /// build on an entry for five minutes, so a mix writes more than it saves.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// Five minutes on every cache entry.
    #[serde(rename = "5m")]
    FiveMinutes,
    /// One hour on every cache entry.
    #[serde(rename = "1h")]
    OneHour,
}

/// `[anthropic]`: the Anthropic provider, with an API key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct AnthropicSettings {
    /// Replaces the Messages API's base URL. Needs a restart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Models added to the provider's model list, such as a model that the API does
    /// not list yet. A prompt may then name them; their efforts are not checked. An
    /// entry is a model id, or a table `{ id, context_window, max_output_tokens }` that
    /// also gives the model's limits in tokens. A table may name a model of the API's
    /// list to lower its window or its output limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelEntry>>,
    /// How long the prompt cache keeps what a request writes: `auto` is efr's choice,
    /// today everything for an hour, `5m` keeps everything for five minutes, `1h` keeps
    /// everything for an hour. An hour costs more per write and survives a longer
    /// pause. Needs a restart.
    pub cache_ttl: CacheTtlChoice,
    /// The workspace of your API key, sent as the `anthropic-workspace-id` header of
    /// every request. A key that is not scoped to one workspace needs it. Needs a
    /// restart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}
