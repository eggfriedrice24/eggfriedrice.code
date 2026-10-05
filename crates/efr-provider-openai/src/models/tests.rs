use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{DEFAULT_SUBSCRIPTION_MODEL, api_models, is_reasoning_model, subscription_models};

#[test]
fn the_default_model_is_listed_for_the_subscription() {
    let models = subscription_models();
    let default = models.iter().find(|model| model.id == DEFAULT_SUBSCRIPTION_MODEL).unwrap();
    assert_eq!(default.context_window, Some(272_000));
    assert_eq!(default.max_output_tokens, Some(128_000));
}

#[test]
fn subscription_models_are_unique_and_reason() {
    let models = subscription_models();
    let mut ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert!(ids.iter().all(|id| is_reasoning_model(id)), "{ids:?}");
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count);
}

#[test]
fn every_subscription_model_names_its_efforts_and_a_default_among_them() {
    for model in subscription_models() {
        assert!(!model.efforts.is_empty(), "{}", model.id);
        let default = model.default_effort.as_deref().unwrap();
        assert!(model.efforts.iter().any(|effort| effort == default), "{}", model.id);
    }
}

#[test]
fn the_subscription_list_is_codexs_listed_catalog() {
    let models: Vec<(String, Vec<String>, Option<String>)> = subscription_models()
        .into_iter()
        .map(|model| (model.id, model.efforts, model.default_effort))
        .collect();
    let ultra = ["low", "medium", "high", "xhigh", "max", "ultra"];
    let max = ["low", "medium", "high", "xhigh", "max"];
    let entry = |id: &str, efforts: &[&str], default: &str| {
        (
            id.to_owned(),
            efforts.iter().map(|effort| (*effort).to_owned()).collect::<Vec<_>>(),
            Some(default.to_owned()),
        )
    };
    assert_eq!(
        models,
        [
            entry("gpt-6.1-sol", &ultra, "low"),
            entry("gpt-6-astra", &ultra, "low"),
            entry("gpt-6-sol", &ultra, "medium"),
            entry("gpt-6-luna", &max, "medium"),
            entry("gpt-5.6-sol", &ultra, "low"),
            entry("gpt-5.6-terra", &ultra, "medium"),
            entry("gpt-5.6-luna", &max, "medium"),
            entry("gpt-5.5", &["low", "medium", "high", "xhigh"], "medium"),
        ]
    );
}

#[test]
fn the_api_lists_no_models() {
    assert!(api_models().is_empty());
}

#[rstest]
#[case("gpt-5.5", true)]
#[case("gpt-6-sol", true)]
#[case("gpt-5-codex", true)]
#[case("o3-mini", true)]
#[case("o4-mini", true)]
#[case("codex-mini-latest", true)]
#[case("gpt-5-chat-latest", false)]
#[case("gpt-4.1", false)]
#[case("gpt-4o", false)]
#[case("", false)]
fn reasoning_families(#[case] model: &str, #[case] reasons: bool) {
    assert_eq!(is_reasoning_model(model), reasons, "{model}");
}
