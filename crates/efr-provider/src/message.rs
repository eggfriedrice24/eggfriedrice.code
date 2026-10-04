//! Canonical messages: the provider-neutral form of a conversation.

use efr_protocol::Base64Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One message of a conversation, in the form every provider converts from and to.
///
/// [`provider_raw`](Self::provider_raw) is native passthrough: the provider's own items
/// for this message (for the Responses API, the `reasoning` item with its encrypted
/// content and the exact `function_call` items), stored verbatim in the event log and
/// sent back unchanged on the next request to the same provider. Only the provider that
/// wrote it reads it; every other crate treats it as opaque. When the conversation
/// switches providers, the history assembler drops it and the next provider works from
/// [`content`](Self::content) alone.
///
/// On the wire, `provider_raw` is left out when absent. A raw value of JSON `null`
/// therefore reads back as absent, which is the same thing to every provider. Object
/// members inside the raw value may come back in a different order (serde_json keeps
/// objects sorted by key), so a provider must not depend on member order; numbers and
/// strings come back exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who wrote the message.
    pub role: Role,
    /// What it says, in order.
    pub content: Vec<ContentBlock>,
    /// The provider's own items for the message, opaque outside that provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_raw: Option<Value>,
}

impl Message {
    /// A message with `content` and no raw items.
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Message { role, content, provider_raw: None }
    }

    /// A user message with one text block.
    pub fn user(text: impl Into<String>) -> Self {
        Message::new(Role::User, vec![ContentBlock::Text { text: text.into() }])
    }

    /// An assistant message with one text block.
    pub fn assistant(text: impl Into<String>) -> Self {
        Message::new(Role::Assistant, vec![ContentBlock::Text { text: text.into() }])
    }

    /// The same message with `raw` as its provider items.
    #[must_use]
    pub fn with_provider_raw(mut self, raw: Value) -> Self {
        self.provider_raw = Some(raw);
        self
    }

    /// The text blocks joined by a blank line, which is how the message reads in a
    /// transcript. Reasoning, tool calls, tool results and images are left out.
    pub fn text(&self) -> String {
        let texts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        texts.join("\n\n")
    }
}

/// Who wrote a message.
///
/// There is no system role: the system prompt is
/// [`Request::system`](crate::Request::system). Tool results travel in a user message,
/// as the user's side of the exchange, and each provider moves them where its API wants
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Role {
    /// The user, or the daemon on the user's behalf (tool results, the live-state
    /// preamble).
    User,
    /// The model.
    Assistant,
}

/// One block of a message.
///
/// Call ids here are the provider's own ids (the Responses API's `call_id`), which tie a
/// tool result to its call. The daemon's `efr_protocol::CallId` is a different id that
/// the conversation maps to these.
///
/// On the wire every block is an object whose `kind` member is the snake_case variant
/// name, such as `{"kind": "text", "text": "hello"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },

    /// The model asks for a tool call.
    ToolCall {
        /// The provider's id of the call.
        call_id: String,
        /// The tool's registered name, such as `shell`.
        name: String,
        /// The tool's input. Arguments that were not valid JSON are kept as a JSON
        /// string, so the tool layer can tell the model what was wrong with them.
        input: Value,
    },

    /// The result of a tool call, sent back to the model.
    ToolResult {
        /// The provider's id of the call this answers.
        call_id: String,
        /// What the model sees: the tool's output, truncated by the tool layer.
        output: String,
        /// True when the tool failed or the call was denied.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },

    /// The model's visible reasoning, such as a reasoning summary. Encrypted or signed
    /// reasoning stays in [`Message::provider_raw`].
    Reasoning {
        /// The reasoning text.
        text: String,
    },

    /// An image, such as a screenshot the user attached.
    Image {
        /// The media type, such as `image/png`.
        media_type: String,
        /// The image bytes, as standard base64 on the wire. `Debug` shows only the
        /// length.
        data: Base64Bytes,
    },
}

#[cfg(test)]
mod tests;
