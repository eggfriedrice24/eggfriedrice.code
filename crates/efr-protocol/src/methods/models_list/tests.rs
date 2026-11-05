use super::{EFFORT_MAX_LEN, ModelInfo, ModelSource, is_effort_word};

fn model(efforts: &[&str]) -> ModelInfo {
    ModelInfo {
        id: "gpt-5.5".to_owned(),
        provider: None,
        efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
        default_effort: None,
        default: false,
        source: ModelSource::Builtin,
        context_window: None,
        max_context_window: None,
        prefer_websockets: false,
    }
}

#[test]
fn a_model_with_known_efforts_takes_only_those() {
    let known = model(&["low", "high"]);
    assert!(known.takes_effort("low"));
    assert!(!known.takes_effort("medium"));
}

#[test]
fn a_model_with_unknown_efforts_takes_any_effort_word() {
    let unknown = model(&[]);
    for effort in ["low", "x-high", "max_2", &"a".repeat(EFFORT_MAX_LEN)] {
        assert!(unknown.takes_effort(effort), "{effort:?}");
        assert!(is_effort_word(effort), "{effort:?}");
    }
    for effort in ["", "High", "very high", "x;y", &"a".repeat(EFFORT_MAX_LEN + 1)] {
        assert!(!unknown.takes_effort(effort), "{effort:?}");
        assert!(!is_effort_word(effort), "{effort:?}");
    }
}

fn of(provider: &str) -> ModelInfo {
    let mut info: ModelInfo =
        serde_json::from_value(serde_json::json!({ "id": "m", "source": "builtin" })).unwrap();
    info.provider = Some(provider.to_owned());
    info
}

#[test]
fn a_prompt_names_a_provider_by_its_id_its_account_or_its_kind() {
    let models = [
        of("openai-subscription"),
        of("openai-subscription.b"),
        of("anthropic-api"),
        of("anthropic-api.work"),
    ];
    let named = |name: &str| super::provider_named(name, &models);

    assert_eq!(named("anthropic-api.work"), Some("anthropic-api.work"));
    assert_eq!(named("work"), Some("anthropic-api.work"));
    assert_eq!(named("b"), Some("openai-subscription.b"));
    assert_eq!(named("anthropic"), Some("anthropic-api"));
    assert_eq!(named("openai"), Some("openai-subscription"));
    assert_eq!(named("openai-api"), None, "no model of it is listed");
    assert_eq!(named("home"), None);
}

#[test]
fn an_account_name_of_two_providers_names_none() {
    let models = [of("anthropic-api.work"), of("openai-api.work")];
    assert_eq!(super::provider_named("work", &models), None);
    assert_eq!(super::provider_named("openai-api.work", &models), Some("openai-api.work"));
}
