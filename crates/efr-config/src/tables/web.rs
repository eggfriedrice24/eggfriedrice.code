//! `[web]`: the web search of the model, which the provider runs inside the model call.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[web]`: when the model can search the web. The provider runs each search inside the
/// model call, so efr cannot ask before one: a search never asks, in any mode. Every key
/// applies from the next turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct WebSettings {
    /// When the model gets the provider's web search. `mode` follows the permission mode
    /// of the turn: off in manual, a stored copy of the web in cautious on an OpenAI
    /// model (a Claude model searches the live web), the live web in auto and full.
    /// `off` never sends the web tool. `cached` asks for the stored copy, which can be
    /// some days old; a Claude model has none and searches the live web. `live` asks for
    /// the live web. In manual mode the model never gets the web tool, whatever this
    /// key says.
    pub search: WebSearch,
    /// The most searches in one turn, from 1 to 100, for a provider that takes a limit
    /// (Claude). Page fetches have the same limit.
    pub max_searches: u32,
}

impl Default for WebSettings {
    fn default() -> Self {
        WebSettings { search: WebSearch::Mode, max_searches: 20 }
    }
}

/// The value of `web.search`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum WebSearch {
    /// Follow the permission mode of the turn.
    #[default]
    Mode,
    /// Never send the web tool.
    Off,
    /// The provider's stored copy of the web where it has one, else the live web.
    Cached,
    /// The live web.
    Live,
}
