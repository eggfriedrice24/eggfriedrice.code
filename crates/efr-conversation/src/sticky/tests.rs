use efr_protocol::{EffectiveSettings, Mode, OverriddenSettings, TurnSettings};

use super::{asked, switched};
use crate::ConversationConfig;

fn ran(model: &str, provider: &str, picked: bool, effort: Option<&str>) -> EffectiveSettings {
    EffectiveSettings {
        mode: Mode::Cautious,
        model: model.to_owned(),
        effort: effort.map(str::to_owned),
        provider: Some(provider.to_owned()),
        overridden: OverriddenSettings { mode: false, model: picked, effort: effort.is_some() },
        fallback: None,
    }
}

fn prompt(model: Option<&str>, effort: Option<&str>) -> TurnSettings {
    TurnSettings { mode: None, model: model.map(str::to_owned), effort: effort.map(str::to_owned) }
}

#[test]
fn a_prompt_without_a_model_keeps_the_pick_of_the_turn_before() {
    let config = ConversationConfig::new("gpt-5.5", "/scratch");
    let before = ran("claude-opus-5-5", "anthropic-api", true, Some("high"));

    let next = asked(&prompt(None, None), Some(&before), &config);

    assert_eq!(next.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(next.effort.as_deref(), Some("high"), "the effort of the pick goes along");
    // The prompt's own effort wins over the pick's.
    let own = asked(&prompt(None, Some("low")), Some(&before), &config);
    assert_eq!(own.effort.as_deref(), Some("low"));
}

#[test]
fn a_prompt_that_names_a_model_or_a_turn_on_the_default_takes_no_pick() {
    let config = ConversationConfig::new("gpt-5.5", "/scratch");
    let picked = ran("claude-opus-5-5", "anthropic-api", true, None);
    let default = ran("gpt-5.5", "openai-api", false, None);

    assert_eq!(
        asked(&prompt(Some("gpt-5.4"), None), Some(&picked), &config).model.as_deref(),
        Some("gpt-5.4")
    );
    assert_eq!(asked(&prompt(None, None), Some(&default), &config).model, None);
    assert_eq!(asked(&prompt(None, None), None, &config).model, None);
}

#[test]
fn a_pick_that_the_model_list_no_longer_has_gives_way_to_the_default() {
    let mut config = ConversationConfig::new("gpt-5.5", "/scratch");
    let mut info: efr_protocol::ModelInfo =
        serde_json::from_value(serde_json::json!({ "id": "gpt-5.5", "source": "builtin" }))
            .unwrap();
    info.provider = Some("openai-api".to_owned());
    config.models = vec![info];
    let before = ran("claude-opus-5-5", "anthropic-api", true, Some("high"));

    let next = asked(&prompt(None, None), Some(&before), &config);

    assert_eq!(next.model, None);
    assert_eq!(next.effort, None);
}

#[test]
fn a_switch_is_another_model_or_another_provider() {
    let gpt = ran("gpt-5.5", "openai-subscription", true, None);
    let claude = ran("claude-opus-5-5", "anthropic-api", true, None);
    let api = ran("gpt-5.5", "openai-api", true, None);
    let mut unknown = ran("gpt-5.5", "x", false, None);
    unknown.provider = None;

    let from = switched(&gpt, &claude).unwrap();
    assert_eq!(
        (from.provider.as_deref(), from.model.as_str()),
        (Some("openai-subscription"), "gpt-5.5")
    );
    assert!(switched(&gpt, &api).is_some(), "the same id at another provider");
    assert!(switched(&gpt, &gpt).is_none());
    assert!(switched(&unknown, &api).is_none(), "an old turn without a provider");
}
