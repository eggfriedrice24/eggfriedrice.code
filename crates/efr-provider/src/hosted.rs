//! Hosted tools: tools that the provider runs inside the model call, such as its web
//! search, and the steps of them that a stream reports.
//!
//! A hosted tool is not a [`ToolDefinition`](crate::ToolDefinition): efr does not run
//! it, the model does not call it through efr, and no result of efr goes back for it. So
//! it rides in its own field, [`Request::hosted`](crate::Request::hosted), and the
//! permission check of the turn never sees it. The turn decides whether a request
//! carries one (the mode and the `[web]` setting), and each provider converts it to its
//! own API.
//!
//! The steps of a hosted tool come back as [`ProviderEvent::HostedCallStart`] and
//! [`ProviderEvent::HostedCallEnd`](crate::ProviderEvent::HostedCallEnd) with a
//! [`HostedAction`] and a [`HostedOutcome`], and the pages that the answer cites as
//! [`ProviderEvent::Citation`](crate::ProviderEvent::Citation). No encrypted content of
//! a provider is in these types: it stays in `provider_raw`.
//!
//! [`ProviderEvent::HostedCallStart`]: crate::ProviderEvent::HostedCallStart

use serde::{Deserialize, Serialize};

/// A tool that the provider runs inside the model call.
///
/// A provider reads each entry of [`Request::hosted`](crate::Request::hosted) and adds
/// its own tool for it; a provider whose [`Provider::web_search`](crate::Provider::web_search)
/// is [`WebSupport::None`] never gets one. On the wire (replay transcripts) an entry is
/// `{"kind": "web_search", "access": "live", ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostedTool {
    /// The web tool of the provider: search, and where the provider has them, the
    /// opening of a page and a find in a page. On Anthropic it is both the web search
    /// and the web fetch tool: the user has one switch for the web.
    WebSearch {
        /// Whether the searches read the provider's stored copy of the web or the live
        /// web.
        access: WebAccess,
        /// The most searches of one turn, for a provider that takes such a limit
        /// (Anthropic's `max_uses`, also for its page fetches). `None` leaves the
        /// provider's own limit.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_uses: Option<u32>,
        /// True when a page fetch of the provider may take a URL from any message of
        /// the conversation: the user's text, which also holds efr's notes, the output
        /// of a background job, a summary and `AGENTS.md`, and the results of efr's own
        /// tools. False when absent: the fetch takes only a URL that a search or an
        /// earlier fetch of the provider found, so a text in a file or in a command's
        /// output cannot make the model load a URL that holds data of the machine.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        urls_from_conversation: bool,
    },
}

impl HostedTool {
    /// The web tool with `access`, a limit of `max_uses` searches when given, and page
    /// fetches only of URLs that a search or a fetch found.
    pub fn web_search(access: WebAccess, max_uses: Option<u32>) -> Self {
        HostedTool::WebSearch { access, max_uses, urls_from_conversation: false }
    }
}

/// Which web a [`HostedTool::WebSearch`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WebAccess {
    /// The provider's stored copy of the web, which can be some days old and fetches
    /// nothing new (OpenAI's `external_web_access: false`). Only a provider with
    /// [`WebSupport::StoredOrLive`] gets it.
    Stored,
    /// The live web.
    Live,
}

/// What a provider can do with [`HostedTool::WebSearch`]
/// ([`Provider::web_search`](crate::Provider::web_search)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WebSupport {
    /// No web tool: a request to this provider never carries one. The default of the
    /// trait, so a fake provider of a test gets none.
    #[default]
    None,
    /// The live web only, such as Anthropic's web search: a request that asks for the
    /// stored copy gets the live web.
    Live,
    /// The stored copy or the live web, such as OpenAI's `web_search`.
    StoredOrLive,
}

/// One step of a hosted web tool, as the model asked for it.
///
/// On the wire (inside a [`ProviderEvent`](crate::ProviderEvent)) an object with an
/// `action` member, such as `{"action": "search", "query": "rust 2026"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostedAction {
    /// A search of the web.
    Search {
        /// The query as the model wrote it, when the provider says it; a start often
        /// lacks it and its end has it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
    },
    /// The opening of one page (OpenAI's `open_page`, Anthropic's web fetch).
    Open {
        /// The page, when the provider says it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// A find of a text in a page that the model opened (OpenAI's `find_in_page`).
    Find {
        /// The text to find.
        pattern: String,
        /// The page, when the provider says it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
}

/// How a step of a hosted tool ended.
///
/// On the wire an object with an `outcome` member, such as
/// `{"outcome": "failed", "error": "too_many_requests"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostedOutcome {
    /// The step ended well.
    Done {
        /// The results of a search when the provider gives them, also an empty list
        /// (the search found nothing); `None` when efr does not know them, such as on
        /// OpenAI and for an open.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        results: Option<Vec<WebResult>>,
    },
    /// The step failed. A failed search is no error of the stream: the model reads the
    /// failure and goes on.
    Failed {
        /// Why.
        error: HostedError,
    },
}

/// One result of a search: a page that the provider found.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WebResult {
    /// The page.
    pub url: String,
    /// Its title, when the provider gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl WebResult {
    /// The result `url` with `title`.
    pub fn new(url: impl Into<String>, title: Option<String>) -> Self {
        WebResult { url: url.into(), title }
    }
}

/// Why a step of a hosted tool failed, in efr's words for every provider. Each provider
/// maps its own codes to these (its README has the table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostedError {
    /// The provider had too many requests (Anthropic's `too_many_requests`).
    TooManyRequests,
    /// The limit of steps of this turn is reached (Anthropic's `max_uses_exceeded`).
    LimitReached,
    /// The tool is not available now (Anthropic's `unavailable`, an OpenAI step with
    /// `status: "failed"`).
    Unavailable,
    /// The provider did not take the input, such as a query or a URL that is too long.
    Invalid,
    /// The page is not allowed, such as a URL that no earlier message held.
    NotAllowed,
    /// The page could not be reached.
    NotAccessible,
    /// The page has a type or a size that the tool does not take.
    Unsupported,
    /// The step did not finish: the answer ended first (an OpenAI step with
    /// `status: "incomplete"`, or one that never got its end).
    NotFinished,
    /// Another reason.
    Other,
}

#[cfg(test)]
mod tests;
