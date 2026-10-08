use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::{ConfigSource as _, HostInfo};
use efr_protocol::Mode;
use jiff::tz::TimeZone;
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use crate::settings::{LiveSettings, conversation_config};

#[test]
fn the_conversation_settings_follow_the_config() {
    let mut config = Settings::default();
    config.model.name = Some("gpt-6-sol".to_owned());
    config.model.max_output_tokens = Some(2048);
    config.conversation.max_queued = 3;
    config.conversation.approval_timeout_secs = Some(90);
    config.conversation.update_interval_ms = 50;
    config.conversation.draft_interval_ms = 8;
    config.model.system_prompt = "be brief".to_owned();
    config.permissions.mode = Mode::Auto;
    let host = HostInfo::new(Some("box".to_owned()), Some("Arch Linux".to_owned()));

    let settings =
        conversation_config(&config, PathBuf::from("/d/scratch"), host.clone(), TimeZone::UTC);

    assert_eq!(settings.model, "gpt-6-sol");
    assert_eq!(settings.scratch_root, PathBuf::from("/d/scratch"));
    assert_eq!(settings.system_prompt.as_deref(), Some("be brief"));
    assert_eq!(settings.max_output_tokens, Some(2048));
    assert_eq!(settings.max_queued, 3);
    assert_eq!(settings.approval_timeout, Some(Duration::from_secs(90)));
    assert_eq!(settings.update_interval, Duration::from_millis(50));
    assert_eq!(settings.draft_interval, Duration::from_millis(8));
    assert_eq!(settings.host, host);
    assert_eq!(settings.mode, Mode::Auto);
    assert_eq!(settings.effort, None);
}

#[test]
fn the_turn_defaults_and_the_model_list_follow_the_config() {
    let mut config = Settings::default();
    config.permissions.mode = Mode::Auto;
    config.model.effort = Some("high".to_owned());
    config.openai.models = Some(vec!["gpt-next".into()]);

    let settings =
        conversation_config(&config, PathBuf::from("/d/s"), HostInfo::default(), TimeZone::UTC);

    assert_eq!(settings.mode, Mode::Auto);
    assert_eq!(settings.effort.as_deref(), Some("high"));
    assert_eq!(settings.models, crate::providers::effective_models(&config));
    assert!(settings.models.iter().any(|model| model.id == "gpt-next"));
}

#[test]
fn the_compaction_settings_reach_the_conversations_with_one_default() {
    assert_eq!(efr_config::DEFAULT_AUTO_AT, efr_conversation::DEFAULT_AUTO_AT);
    let mut config = Settings::default();
    let defaults =
        conversation_config(&config, PathBuf::from("/d/s"), HostInfo::default(), TimeZone::UTC);
    assert_eq!(defaults.compaction, efr_conversation::CompactionConfig::default());

    config.compaction.auto = false;
    config.compaction.auto_at = 60;
    let settings =
        conversation_config(&config, PathBuf::from("/d/s"), HostInfo::default(), TimeZone::UTC);

    assert_eq!(settings.compaction, efr_conversation::CompactionConfig::new(false, 60));
}

#[test]
fn the_conversations_read_the_latest_settings() {
    let (sender, receiver) = watch::channel(Arc::new(Settings::default()));
    let host = HostInfo::new(Some("box".to_owned()), None);
    let live = LiveSettings::new(receiver, PathBuf::from("/d/scratch"), host, TimeZone::UTC);

    let before = live.current();
    assert_eq!(before.model, efr_provider_openai::DEFAULT_SUBSCRIPTION_MODEL);
    assert_eq!(before.system_prompt.as_deref(), Some(efr_config::DEFAULT_SYSTEM_PROMPT));
    let mut changed = Settings::default();
    changed.model.name = Some("gpt-6-sol".to_owned());
    changed.conversation.max_queued = 2;
    changed.permissions.mode = Mode::Manual;
    sender.send_replace(Arc::new(changed));

    let after = live.current();
    assert_eq!(after.model, "gpt-6-sol");
    assert_eq!(after.max_queued, 2);
    assert_eq!(before.mode, Mode::Cautious);
    assert_eq!(after.mode, Mode::Manual);
    assert_eq!(after.scratch_root, PathBuf::from("/d/scratch"));
    assert_eq!(before.model, efr_provider_openai::DEFAULT_SUBSCRIPTION_MODEL, "a value read stays");
}
