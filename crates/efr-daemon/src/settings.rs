//! The settings as the conversations read them: the latest value of the daemon's
//! settings watch and the current model list of every provider with a login
//! (`catalogs.rs`), turned into a `ConversationConfig` each time a turn starts or a
//! prompt arrives, with whether full mode may run. Both come from memory, so a turn
//! never waits for a fetch of a catalog.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::{
    CompactionConfig, ConfigSource, ConversationConfig, ForeignDefault, HostInfo, WebConfig,
};
use jiff::tz::TimeZone;
use tokio::sync::watch;

use crate::catalogs::Catalogs;

/// The conversations' view of the settings watch and the model catalog, with the facts
/// that do not change while the daemon runs.
#[derive(Debug)]
pub(crate) struct LiveSettings {
    settings: watch::Receiver<Arc<Settings>>,
    scratch_root: PathBuf,
    host: HostInfo,
    time_zone: TimeZone,
    catalogs: Arc<Catalogs>,
    /// True when efrd runs as root, which refuses full mode.
    root: bool,
}

impl LiveSettings {
    /// Reads `settings` and the lists of `catalogs`, with scratch directories under
    /// `scratch_root`.
    pub(crate) fn new(
        settings: watch::Receiver<Arc<Settings>>,
        scratch_root: PathBuf,
        host: HostInfo,
        time_zone: TimeZone,
        catalogs: Arc<Catalogs>,
    ) -> Self {
        LiveSettings { settings, scratch_root, host, time_zone, catalogs, root: false }
    }

    /// Says whether efrd runs as root, which refuses full mode.
    pub(crate) fn with_root(mut self, root: bool) -> Self {
        self.root = root;
        self
    }
}

impl ConfigSource for LiveSettings {
    fn current(&self) -> Arc<ConversationConfig> {
        // NOTE: the settings are cloned out so the watch's read lock is held only here.
        let settings = Arc::clone(&self.settings.borrow());
        let mut config = conversation_config(
            &settings,
            &self.catalogs,
            self.scratch_root.clone(),
            self.host.clone(),
            self.time_zone.clone(),
        );
        // NOTE: from the latest settings, so a reload that turns undo off refuses full
        // mode from the next resolve and the next call on (DF16).
        config.full_mode = crate::full::status(&settings, self.root);
        Arc::new(config)
    }
}

/// The conversations' settings from `settings`, with the current model list of
/// `catalogs`.
pub(crate) fn conversation_config(
    settings: &Settings,
    catalogs: &Catalogs,
    scratch_root: PathBuf,
    host: HostInfo,
    time_zone: TimeZone,
) -> ConversationConfig {
    let configured = catalogs.configured(settings);
    let mut config = ConversationConfig::new(configured.default_model(settings), scratch_root)
        .with_system_prompt(settings.model.system_prompt.clone())
        .with_time_zone(time_zone)
        .with_host(host);
    config.effort.clone_from(&settings.model.effort);
    for (provider, effort) in [
        (crate::providers::SUBSCRIPTION, &settings.openai.effort),
        (crate::providers::API, &settings.openai.effort),
        (crate::providers::ANTHROPIC, &settings.anthropic.effort),
    ] {
        if let Some(effort) = effort {
            config.provider_efforts.insert(provider.to_owned(), effort.clone());
        }
    }
    config.mode = settings.permissions.mode;
    config.models = catalogs.routed(settings);
    config.max_output_tokens = settings.model.max_output_tokens;
    config.compaction =
        CompactionConfig::new(settings.compaction.auto, settings.compaction.auto_at);
    config.max_queued = settings.conversation.max_queued;
    config.approval_timeout = settings.conversation.approval_timeout_secs.map(Duration::from_secs);
    config.classifier_enabled = settings.classifier.enabled;
    config.away_wait = Duration::from_secs(u64::from(settings.presence.away_wait_secs));
    config.max_wakes_per_hour = settings.jobs.max_wakes_per_hour;
    config.update_interval = Duration::from_millis(settings.conversation.update_interval_ms);
    // NOTE: the drafts of a turn go at once. `conversation.draft_interval_ms` is the
    // gate of a remote subscriber, which `conversation.subscribe` applies.
    config.recent_commands = usize::try_from(settings.context.recent_commands).unwrap_or(0);
    config.web = WebConfig::new(web_search(settings.web.search), settings.web.max_searches);
    // NOTE: a default of another company goes to its provider when one with a login
    // lists it; it fails only without one.
    config.foreign_model = settings
        .foreign_model()
        .filter(|foreign| !served(catalogs, &config.models, foreign.name))
        .map(|foreign| {
            ForeignDefault::new(
                foreign.name,
                foreign.company.name(),
                foreign.provider,
                foreign.company.login(),
            )
        });
    config
}

/// The conversations' value of `web.search`. A value that this build of efrd does not
/// know sends no web tool.
fn web_search(search: efr_config::WebSearch) -> efr_conversation::WebSearch {
    match search {
        efr_config::WebSearch::Mode => efr_conversation::WebSearch::Mode,
        efr_config::WebSearch::Cached => efr_conversation::WebSearch::Cached,
        efr_config::WebSearch::Live => efr_conversation::WebSearch::Live,
        _ => efr_conversation::WebSearch::Off,
    }
}

/// True when a provider with a login lists `name` in `models`.
pub(crate) fn served(catalogs: &Catalogs, models: &[efr_protocol::ModelInfo], name: &str) -> bool {
    models.iter().any(|info| {
        info.id == name
            && info.provider.as_deref().is_some_and(|provider| catalogs.logged_in(provider))
    })
}

#[cfg(test)]
mod tests;
