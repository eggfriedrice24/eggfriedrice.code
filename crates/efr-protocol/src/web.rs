//! The web search of the model, which the provider runs inside the model call: the
//! kinds of a step, why a step failed, a result and a cited page.
//!
//! The events are [`Event::WebCallStarted`](crate::Event::WebCallStarted),
//! [`Event::WebCallCompleted`](crate::Event::WebCallCompleted) and
//! [`Event::TurnWebReport`](crate::Event::TurnWebReport). efr does not run a web step
//! and never asks before one, so they are not `tool_call_*` events.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most results that one [`Event::WebCallCompleted`](crate::Event::WebCallCompleted)
/// holds. The daemon keeps the first ones.
pub const MAX_WEB_RESULTS: usize = 20;

/// The most cited pages that one [`Event::TurnWebReport`](crate::Event::TurnWebReport)
/// holds. The daemon keeps the first ones.
pub const MAX_WEB_SOURCES: usize = 50;

/// The most characters of a query or a pattern in an event of a web step. The daemon
/// cuts a longer one.
pub const MAX_WEB_TEXT_CHARS: usize = 1_000;

/// The most characters of a URL in an event of a web step. The daemon cuts a longer
/// one.
pub const MAX_WEB_URL_CHARS: usize = 2_048;

/// The most characters of a page title in an event of a web step. The daemon cuts a
/// longer one.
pub const MAX_WEB_TITLE_CHARS: usize = 200;

/// What a step of the web tool does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WebAction {
    /// A search of the web, with its `query`.
    Search,
    /// The opening of one page, its `url`.
    Open,
    /// A find of a text (`pattern`) in a page that the model opened (`url`). A client
    /// shows no row of its own for it.
    Find,
    /// An action that this build does not know.
    #[serde(other)]
    Unknown,
}

impl WebAction {
    /// Every action, in the order of the enum.
    pub const ALL: [WebAction; 4] =
        [WebAction::Search, WebAction::Open, WebAction::Find, WebAction::Unknown];

    /// The wire form, such as `search`.
    pub const fn as_str(self) -> &'static str {
        match self {
            WebAction::Search => "search",
            WebAction::Open => "open",
            WebAction::Find => "find",
            WebAction::Unknown => "unknown",
        }
    }
}

/// Why a step of the web tool failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WebError {
    /// The provider had too many requests.
    TooManyRequests,
    /// The limit of searches or page fetches of the turn is reached.
    LimitReached,
    /// The web tool of the provider is not available now.
    Unavailable,
    /// The provider did not take the input, such as a query or a URL that is too long.
    Invalid,
    /// The page is not allowed, such as a URL that no earlier message held.
    NotAllowed,
    /// The page could not be reached.
    NotAccessible,
    /// The page has a type or a size that the tool does not take.
    Unsupported,
    /// The step did not finish: the answer or the turn ended first.
    NotFinished,
    /// A reason that this build does not know, or another reason.
    #[serde(other)]
    Unknown,
}

impl WebError {
    /// Every reason, in the order of the enum.
    pub const ALL: [WebError; 9] = [
        WebError::TooManyRequests,
        WebError::LimitReached,
        WebError::Unavailable,
        WebError::Invalid,
        WebError::NotAllowed,
        WebError::NotAccessible,
        WebError::Unsupported,
        WebError::NotFinished,
        WebError::Unknown,
    ];

    /// The wire form, such as `too_many_requests`.
    pub const fn as_str(self) -> &'static str {
        match self {
            WebError::TooManyRequests => "too_many_requests",
            WebError::LimitReached => "limit_reached",
            WebError::Unavailable => "unavailable",
            WebError::Invalid => "invalid",
            WebError::NotAllowed => "not_allowed",
            WebError::NotAccessible => "not_accessible",
            WebError::Unsupported => "unsupported",
            WebError::NotFinished => "not_finished",
            WebError::Unknown => "unknown",
        }
    }
}

/// One result of a search: a page that the provider found.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WebResult {
    /// The page, at most [`MAX_WEB_URL_CHARS`] characters.
    pub url: String,
    /// Its title, at most [`MAX_WEB_TITLE_CHARS`] characters. Absent when the provider
    /// gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A page that the answer of a turn cites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WebSource {
    /// The page, at most [`MAX_WEB_URL_CHARS`] characters.
    pub url: String,
    /// Its title, at most [`MAX_WEB_TITLE_CHARS`] characters. Absent when the provider
    /// gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}
