//! The provider boundary.

use std::fmt;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::{Completion, ProviderError, ProviderEvent, ProviderId, Request};

/// The stream a [`Provider`] answers with. It is boxed and `Send` so that the trait
/// stays dyn-compatible and the conversation can poll it from any task.
pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

/// A way of reaching a model: the OpenAI subscription, an OpenAI API key, an Anthropic
/// API key, and the replay provider in tests.
///
/// [`stream`](Provider::stream) is the one model call a provider implements;
/// [`complete`](Provider::complete) collects it for callers that want the whole answer,
/// the shape goose uses. The trait is dyn-compatible (through `async-trait`) because
/// the daemon composes providers from credential rows at run time and hands the
/// conversation an `Arc<dyn Provider>`.
///
/// A provider takes its credentials from a [`TokenSource`](crate::TokenSource) and
/// never learns how a token was obtained. On a 401 it calls
/// [`TokenSource::invalidate`](crate::TokenSource::invalidate), retries once, and on a
/// second 401 fails with [`ProviderError::Unauthorized`].
#[async_trait]
pub trait Provider: Send + Sync + fmt::Debug {
    /// The provider's configured name, for logs, the event log and deciding whether a
    /// message's `provider_raw` belongs to it.
    fn id(&self) -> &ProviderId;

    /// The models the provider knows it can serve now. Empty when it does not say, in
    /// which case any model id is passed through and the provider's answer decides. The
    /// list can change while the provider runs, such as when a new model catalog comes
    /// from the backend, so a caller gets its own copy.
    fn models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    /// Sends `request` and returns the answer as it streams. An error here means the
    /// request failed before any event; later failures arrive as error items.
    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError>;

    /// Sends `request` and collects the whole answer with [`Completion::collect`].
    async fn complete(&self, request: Request) -> Result<Completion, ProviderError> {
        Completion::collect(self.stream(request).await?).await
    }
}

/// What a provider says about one model it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelInfo {
    /// The model id a [`Request`] names.
    pub id: String,
    /// The context window in tokens, when known: the window that a conversation uses
    /// by default.
    pub context_window: Option<u64>,
    /// The largest context window in tokens that a setting can raise the window to,
    /// when known. A larger setting is cut down to it.
    pub max_context_window: Option<u64>,
    /// The output token limit, when known.
    pub max_output_tokens: Option<u32>,
    /// The reasoning efforts the model takes, such as `low` and `high`, in the order a
    /// picker shows them. Empty when they are not known, and then any effort is passed
    /// through and the backend's answer decides.
    pub efforts: Vec<String>,
    /// The effort the backend uses when a request names none, when known.
    pub default_effort: Option<String>,
    /// True when the model takes freeform tools, so a tool with a grammar goes to it in
    /// its freeform form. False sends every freeform tool in its function form, which
    /// every model with function calls takes.
    pub freeform_tools: bool,
    /// True when the backend prefers that a client reach this model over a WebSocket
    /// and not over a streamed HTTP response. It is a fact for the transport to read;
    /// a provider without such a transport ignores it.
    pub prefer_websockets: bool,
    /// The tool with which the model changes a file. The code that builds a request's
    /// tool list offers the model this one and not the other.
    pub edit_tool: EditTool,
}

/// The tool with which a model changes a file: the form that it was trained on.
///
/// A request offers a model exactly one of them, with `write_file` and `read_file`
/// beside it. The choice follows the model, so the tool list and the prompt cache stay
/// the same from call to call while the model stays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EditTool {
    /// `apply_patch`: a patch in the Codex format, which can change, add, delete and
    /// move several files in one call. OpenAI's models know it.
    #[default]
    ApplyPatch,
    /// `edit`: the exact text `old_string` in one file becomes `new_string`, once or
    /// at every place (`replace_all`), as Claude Code's edit tool does. Anthropic's
    /// models know it.
    Replace,
}

impl ModelInfo {
    /// A model with no known limits, which changes files with
    /// [`EditTool::ApplyPatch`].
    pub fn new(id: impl Into<String>) -> Self {
        ModelInfo {
            id: id.into(),
            context_window: None,
            max_context_window: None,
            max_output_tokens: None,
            efforts: Vec::new(),
            default_effort: None,
            freeform_tools: false,
            prefer_websockets: false,
            edit_tool: EditTool::ApplyPatch,
        }
    }

    /// The same model, which changes files with `edit_tool`.
    #[must_use]
    pub fn with_edit_tool(mut self, edit_tool: EditTool) -> Self {
        self.edit_tool = edit_tool;
        self
    }

    /// The same model with a known context window.
    #[must_use]
    pub fn with_context_window(mut self, tokens: u64) -> Self {
        self.context_window = Some(tokens);
        self
    }

    /// The same model with the largest window that a setting can raise its window to.
    #[must_use]
    pub fn with_max_context_window(mut self, tokens: u64) -> Self {
        self.max_context_window = Some(tokens);
        self
    }

    /// The same model, which takes freeform tools when `takes` is true.
    #[must_use]
    pub fn with_freeform_tools(mut self, takes: bool) -> Self {
        self.freeform_tools = takes;
        self
    }

    /// The same model with the backend's preference for a WebSocket transport.
    #[must_use]
    pub fn with_prefer_websockets(mut self, prefers: bool) -> Self {
        self.prefer_websockets = prefers;
        self
    }

    /// The same model with a known output token limit.
    #[must_use]
    pub fn with_max_output_tokens(mut self, tokens: u32) -> Self {
        self.max_output_tokens = Some(tokens);
        self
    }

    /// The same model with the reasoning `efforts` it takes and the backend's
    /// `default_effort` for it.
    #[must_use]
    pub fn with_efforts<I, S>(mut self, efforts: I, default_effort: Option<&str>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.efforts = efforts.into_iter().map(Into::into).collect();
        self.default_effort = default_effort.map(str::to_owned);
        self
    }
}

#[cfg(test)]
mod tests;
