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
