use pretty_assertions::assert_eq;
use rstest::rstest;

use super::is_reasoning_model;
use crate::{Backend, Catalog};

/// A model as the table test compares it: id, efforts, default effort, largest window.
type Row = (String, Vec<String>, Option<String>, Option<u64>);

#[test]
fn the_builtin_table_is_codexs_listed_catalog_in_priority_order() {
    let models: Vec<Row> = Catalog::builtin(Backend::Subscription)
        .models()
        .into_iter()
        .map(|model| (model.id, model.efforts, model.default_effort, model.max_context_window))
        .collect();
    let ultra = ["low", "medium", "high", "xhigh", "max", "ultra"];
    let max = ["low", "medium", "high", "xhigh", "max"];
    let entry = |id: &str, efforts: &[&str], default: &str, most: u64| {
        (
            id.to_owned(),
            efforts.iter().map(|effort| (*effort).to_owned()).collect::<Vec<_>>(),
            Some(default.to_owned()),
            Some(most),
        )
    };
    assert_eq!(
        models,
        [
            entry("gpt-6.1-sol", &ultra, "low", 872_000),
            entry("gpt-6-astra", &ultra, "low", 872_000),
            entry("gpt-6-sol", &ultra, "medium", 872_000),
            entry("gpt-6-luna", &max, "medium", 872_000),
            entry("gpt-5.6-sol", &ultra, "low", 872_000),
            entry("gpt-5.6-terra", &ultra, "medium", 872_000),
            entry("gpt-5.6-luna", &max, "medium", 872_000),
            entry("gpt-5.5", &["low", "medium", "high", "xhigh"], "medium", 272_000),
        ]
    );
}

#[test]
fn the_builtin_default_is_the_newest_workhorse_and_not_the_legacy_model() {
    assert_eq!(
        Catalog::builtin(Backend::Subscription).default_model().as_deref(),
        Some("gpt-6.1-sol")
    );
}

#[test]
fn every_builtin_model_reasons_takes_freeform_tools_and_has_a_window() {
    let models = Catalog::builtin(Backend::Subscription).models();
    let mut ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    for model in &models {
        assert!(is_reasoning_model(&model.id), "{}", model.id);
        assert!(model.freeform_tools, "{}", model.id);
        assert!(model.prefer_websockets, "{}", model.id);
        assert_eq!(model.context_window, Some(272_000), "{}", model.id);
        let default = model.default_effort.as_deref().unwrap();
        assert!(model.efforts.iter().any(|effort| effort == default), "{}", model.id);
    }
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count);
}

#[test]
fn the_api_backend_falls_back_to_the_same_table() {
    assert_eq!(
        Catalog::builtin(Backend::Api).models(),
        Catalog::builtin(Backend::Subscription).models()
    );
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
