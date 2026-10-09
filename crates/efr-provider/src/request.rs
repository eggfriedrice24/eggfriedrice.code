//! One request to a model, in canonical form.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

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
    /// The system prompt, the static rules of every call. Each provider puts it where
    /// its API takes it.
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
    /// How hard the model reasons, such as `low`, `medium` or `high`: one of the
    /// [`ModelInfo::efforts`](crate::ModelInfo::efforts) of the model. The provider's
    /// default for the model when absent. Every provider reads it and maps it to its
    /// own field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Options only one provider understands, such as OpenAI's `prompt_cache_key`,
    /// passed through as they are. A provider ignores keys it does not know, and never
    /// sends a key it does not know to its API.
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
///
/// A tool is a function tool, whose input is a JSON object that
/// [`input_schema`](Self::input_schema) describes, or a freeform tool, whose input is
/// plain text that [`grammar`](Self::grammar) describes, such as a patch for
/// `apply_patch`. A freeform tool keeps the schema of its function form in
/// `input_schema`: one string member [`FREEFORM_INPUT`] that holds the text. Each
/// provider sends a freeform tool in its freeform form only to a model that takes that
/// form, and in its function form to every other model. The model's call then has the
/// text as [`ContentBlock::ToolCall`](crate::ContentBlock::ToolCall) `input`, as a JSON
/// string with `freeform` set, or as `{"input": "<text>"}` from the function form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name the model calls the tool by, such as `shell`.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// The JSON Schema of the tool's input object. For a freeform tool, the schema of
    /// its function form.
    pub input_schema: Value,
    /// The grammar of a freeform tool's text input; absent for a function tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammar: Option<ToolGrammar>,
}

impl ToolDefinition {
    /// A function tool whose input is the JSON object that `input_schema` describes.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            input_schema,
            grammar: None,
        }
    }

    /// A freeform tool whose text input `grammar` describes. Its function form takes
    /// the text in one string member, [`FREEFORM_INPUT`].
    pub fn freeform(
        name: impl Into<String>,
        description: impl Into<String>,
        grammar: ToolGrammar,
    ) -> Self {
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            input_schema: freeform_input_schema(),
            grammar: Some(grammar),
        }
    }

    /// True for a freeform tool.
    pub fn is_freeform(&self) -> bool {
        self.grammar.is_some()
    }
}

/// The member that holds the text when a freeform tool is sent in its function form.
pub const FREEFORM_INPUT: &str = "input";

/// The JSON Schema of a freeform tool's function form: an object with one required
/// string member, [`FREEFORM_INPUT`].
pub fn freeform_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            FREEFORM_INPUT: {
                "type": "string",
                "description": "The whole input of the tool as plain text, in the format that the tool's description gives.",
            },
        },
        "required": [FREEFORM_INPUT],
        "additionalProperties": false,
    })
}

/// The grammar of a freeform tool's text input, as the Responses API takes it in a
/// `custom` tool's `format`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrammar {
    /// The language the definition is written in.
    pub syntax: GrammarSyntax,
    /// The grammar itself.
    pub definition: String,
}

impl ToolGrammar {
    /// A Lark grammar.
    pub fn lark(definition: impl Into<String>) -> Self {
        ToolGrammar { syntax: GrammarSyntax::Lark, definition: definition.into() }
    }
}

/// The language of a [`ToolGrammar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GrammarSyntax {
    /// A Lark context-free grammar.
    Lark,
    /// A regular expression.
    Regex,
}

#[cfg(test)]
mod tests;
