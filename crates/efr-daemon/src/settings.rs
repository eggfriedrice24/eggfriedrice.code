//! The settings as the conversations read them: the latest value of the daemon's
//! settings watch and the current model catalog, turned into a `ConversationConfig`
//! each time a turn starts or a prompt arrives, with whether full mode may run. Both
//! come from memory, so a turn never waits for a fetch of the catalog.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::{
    CompactionConfig, ConfigSource, ConversationConfig, ForeignDefault, HostInfo,
};
use jiff::tz::TimeZone;
use tokio::sync::watch;

use crate::catalog::Models;

/// The conversations' view of the settings watch and the model catalog, with the facts
/// that do not change while the daemon runs.
#[derive(Debug)]
pub(crate) struct LiveSettings {
    settings: watch::Receiver<Arc<Settings>>,
    scratch_root: PathBuf,
    host: HostInfo,
    time_zone: TimeZone,
    models: Arc<Models>,
    /// True when efrd runs as root, which refuses full mode.
    root: bool,
}

impl LiveSettings {
    /// Reads `settings` and `models`, with scratch directories under `scratch_root`.
    pub(crate) fn new(
        settings: watch::Receiver<Arc<Settings>>,
        scratch_root: PathBuf,
        host: HostInfo,
        time_zone: TimeZone,
        models: Arc<Models>,
    ) -> Self {
        LiveSettings { settings, scratch_root, host, time_zone, models, root: false }
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
            &self.models,
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

/// The conversations' settings from `settings`, with the current list of `models`.
pub(crate) fn conversation_config(
    settings: &Settings,
    models: &Models,
    scratch_root: PathBuf,
    host: HostInfo,
    time_zone: TimeZone,
) -> ConversationConfig {
    let mut config = ConversationConfig::new(models.default_model(settings), scratch_root)
        .with_system_prompt(settings.model.system_prompt.clone())
        .with_time_zone(time_zone)
        .with_host(host);
    config.effort.clone_from(&settings.model.effort);
    config.mode = settings.permissions.mode;
    config.models = models.effective(settings);
    config.max_output_tokens = settings.model.max_output_tokens;
    config.compaction =
        CompactionConfig::new(settings.compaction.auto, settings.compaction.auto_at);
    config.max_queued = settings.conversation.max_queued;
    config.approval_timeout = settings.conversation.approval_timeout_secs.map(Duration::from_secs);
    config.classifier_enabled = settings.classifier.enabled;
    config.away_wait = Duration::from_secs(u64::from(settings.presence.away_wait_secs));
    config.max_wakes_per_hour = settings.jobs.max_wakes_per_hour;
    config.update_interval = Duration::from_millis(settings.conversation.update_interval_ms);
    config.draft_interval = Duration::from_millis(settings.conversation.draft_interval_ms);
    config.recent_commands = usize::try_from(settings.context.recent_commands).unwrap_or(0);
    config.foreign_model = settings
        .foreign_model()
        .map(|foreign| ForeignDefault::new(foreign.name, foreign.company.name(), foreign.provider));
    config
}

#[cfg(test)]
mod tests;
