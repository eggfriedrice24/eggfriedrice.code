//! The gate of web search: whether a turn's requests carry the provider's web tool, and
//! with which access.
//!
//! A hosted search runs inside the model call, so it never passes
//! `authorize_tool_call` and efr cannot ask before it. The gate is therefore where the
//! request is built: [`plan`] reads the effective mode of the turn, the `web.search`
//! setting and what the provider supports, and [`hosted_tools`] turns the plan into the
//! [`Request::hosted`](efr_provider::Request::hosted) list. `Turn::drive` sets the list
//! of the turn's base request once, so it stays the same for the whole turn; the
//! compaction and the context view build the same list through [`hosted_for`], so their
//! token counts and the prompt cache see the request of the turn.
//!
//! The order of the plan (the build spec of web search, 3.5):
//!
//! 1. a provider without support gives off;
//! 2. `manual` gives off, whatever the setting says: in manual every action of the
//!    model asks, and a hosted search cannot ask;
//! 3. `off` gives off;
//! 4. `live` gives the live web;
//! 5. `cached` gives the stored copy where the provider has one, else the live web;
//! 6. `mode` gives the stored copy in `cautious` where the provider has one, and the
//!    live web in every other case.
//!
//! The module does no IO.

use efr_protocol::{EffectiveSettings, Mode};
use efr_provider::{HostedTool, Provider, WebAccess, WebSupport};

use crate::ConversationConfig;

/// The most searches of one turn when the config names none (`web.max_searches`).
pub const DEFAULT_MAX_SEARCHES: u32 = 20;

/// The value of `web.search`: when a turn's requests carry the provider's web tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WebSearch {
    /// Follow the permission mode of the turn: off in `manual`, the stored copy in
    /// `cautious` on a provider that has one, the live web otherwise.
    #[default]
    Mode,
    /// Never send the web tool.
    Off,
    /// The provider's stored copy of the web where it has one, else the live web. Off
    /// in `manual`.
    Cached,
    /// The live web. Off in `manual`.
    Live,
}

/// The `[web]` settings of a conversation ([`ConversationConfig::web`]). A turn reads
/// them when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct WebConfig {
    /// `web.search`.
    pub search: WebSearch,
    /// `web.max_searches`: the most searches of one turn on a provider that takes a
    /// limit.
    pub max_searches: u32,
}

impl WebConfig {
    /// The settings `search` with a limit of `max_searches` searches in a turn.
    pub fn new(search: WebSearch, max_searches: u32) -> Self {
        WebConfig { search, max_searches }
    }
}

impl Default for WebConfig {
    /// `mode`, and [`DEFAULT_MAX_SEARCHES`].
    fn default() -> Self {
        WebConfig::new(WebSearch::Mode, DEFAULT_MAX_SEARCHES)
    }
}

/// What a turn does with the web tool. The turn keeps it in `Turn::web_plan`: the
/// slice of the turn reads it for the line of the preamble and for the stored copy of
/// its report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WebPlan {
    /// The request carries no web tool, for this reason.
    Off(WhyOff),
    /// The provider's stored copy of the web.
    Stored,
    /// The live web.
    Live,
}

/// Why a turn's requests carry no web tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WhyOff {
    /// The provider has no web tool.
    NoSupport,
    /// The turn runs in `manual`.
    Manual,
    /// `web.search = "off"`.
    Setting,
}

/// The plan of a turn in `mode` (the effective mode: an `auto` turn that fell back runs
/// as `cautious`), with the setting `setting`, on a provider with `support`.
pub(crate) fn plan(mode: Mode, setting: WebSearch, support: WebSupport) -> WebPlan {
    let stored = || match support {
        WebSupport::StoredOrLive => WebPlan::Stored,
        _ => WebPlan::Live,
    };
    if support == WebSupport::None {
        return WebPlan::Off(WhyOff::NoSupport);
    }
    if mode == Mode::Manual {
        return WebPlan::Off(WhyOff::Manual);
    }
    match setting {
        WebSearch::Off => WebPlan::Off(WhyOff::Setting),
        WebSearch::Live => WebPlan::Live,
        WebSearch::Cached => stored(),
        WebSearch::Mode => match mode {
            Mode::Cautious => stored(),
            _ => WebPlan::Live,
        },
    }
}

/// The hosted tools of a request of a turn with `plan` in `mode`: nothing when the plan
/// is off, else the web tool with its access, a limit of `max` searches, and page
/// fetches of URLs from efr's own tool results only in `full` (where every command
/// runs with no question already).
pub(crate) fn hosted_tools(plan: WebPlan, mode: Mode, max: u32) -> Vec<HostedTool> {
    let access = match plan {
        WebPlan::Off(_) => return Vec::new(),
        WebPlan::Stored => WebAccess::Stored,
        WebPlan::Live => WebAccess::Live,
    };
    vec![HostedTool::WebSearch {
        access,
        max_uses: Some(max),
        urls_from_tool_results: mode == Mode::Full,
    }]
}

/// The hosted tools of a request with the settings `settings` of a turn, the config
/// `config` and `provider`: what a turn with these settings sends. The compaction and
/// the context view use it, so their requests match the turn's.
pub(crate) fn hosted_for(
    settings: &EffectiveSettings,
    config: &ConversationConfig,
    provider: &dyn Provider,
) -> Vec<HostedTool> {
    let plan = plan(settings.mode, config.web.search, provider.web_search());
    hosted_tools(plan, settings.mode, config.web.max_searches)
}

#[cfg(test)]
mod tests;
