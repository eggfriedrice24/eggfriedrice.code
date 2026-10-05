//! The settings as the conversations read them: the latest value of the daemon's
//! settings watch, turned into a `ConversationConfig` each time a turn starts or a
//! prompt arrives.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::{ConfigSource, ConversationConfig, HostInfo};
use jiff::tz::TimeZone;
use tokio::sync::watch;

use crate::providers::{default_model, effective_models};

/// The conversations' view of the settings watch, with the facts that do not change
/// while the daemon runs.
#[derive(Debug)]
pub(crate) struct LiveSettings {
    settings: watch::Receiver<Arc<Settings>>,
    scratch_root: PathBuf,
    host: HostInfo,
    time_zone: TimeZone,
}

impl LiveSettings {
    /// Reads `settings`, with scratch directories under `scratch_root`.
    pub(crate) fn new(
        settings: watch::Receiver<Arc<Settings>>,
        scratch_root: PathBuf,
        host: HostInfo,
        time_zone: TimeZone,
    ) -> Self {
        LiveSettings { settings, scratch_root, host, time_zone }
    }
}

impl ConfigSource for LiveSettings {
    fn current(&self) -> Arc<ConversationConfig> {
        // NOTE: the settings are cloned out so the watch's read lock is held only here.
        let settings = Arc::clone(&self.settings.borrow());
        Arc::new(conversation_config(
            &settings,
            self.scratch_root.clone(),
            self.host.clone(),
            self.time_zone.clone(),
        ))
    }
}

/// The conversations' settings from `settings`.
pub(crate) fn conversation_config(
    settings: &Settings,
    scratch_root: PathBuf,
    host: HostInfo,
    time_zone: TimeZone,
) -> ConversationConfig {
    let mut config = ConversationConfig::new(default_model(settings), scratch_root)
        .with_system_prompt(settings.model.system_prompt.clone())
        .with_time_zone(time_zone)
        .with_host(host);
    config.effort.clone_from(&settings.model.effort);
    config.mode = settings.permissions.mode;
    config.models = effective_models(settings);
    config.max_output_tokens = settings.model.max_output_tokens;
    config.max_queued = settings.conversation.max_queued;
    config.approval_timeout = settings.conversation.approval_timeout_secs.map(Duration::from_secs);
    config.update_interval = Duration::from_millis(settings.conversation.update_interval_ms);
    config
}

#[cfg(test)]
mod tests;
