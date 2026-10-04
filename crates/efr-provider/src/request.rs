//! One request to a model, in canonical form.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Message;

/// Everything a provider needs for one model call.
///
/// The conversation builds it from the event log and the tool registry; the provider
/// converts it to its own API. It is serde so that the replay harness can record a
/// request and compare the next one against it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The provider's model id, such as `gpt-5-codex`.
    pub model: String,
    /// The system prompt, which the Responses API calls `instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// The conversation so far, oldest first.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// The tools the model may call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// The most tokens the model may produce, reasoning included; the provider's
    /// default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Options only one provider understands, such as a reasoning effort, passed
    /// through as they are. A provider ignores keys it does not know.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub provider_options: Map<String, Value>,
}

impl Request {
    /// A request for `model` with no messages, no tools and no options.
    pub fn new(model: impl Into<String>) -> Self {
        Request { model: model.into(), ..Request::default() }
    }
}

/// A tool as the model sees it.
///
/// `efr-tools` describes its tools with its own `ToolSpec`; the conversation converts
/// each into this type, because this crate and `efr-tools` do not depend on each
/// other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name the model calls the tool by, such as `shell`.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// The JSON Schema of the tool's input object.
    pub input_schema: Value,
}

#[cfg(test)]
mod tests;
