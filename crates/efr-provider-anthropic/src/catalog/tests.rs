use std::sync::Arc;

use efr_provider::{EditTool, ModelInfo};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{Applied, Catalog, CatalogOrigin, ModelCatalog, entries_of, with_extra};
use crate::testing::{model_entry, start};
use crate::{DEFAULT_EFFORT, DEFAULT_MODEL};

fn catalog_of(list: &Value) -> Catalog {
    let (entries, broken) = entries_of(list);
    let listed = entries.len() + broken;
    Catalog::from_backend("https://api.anthropic.test/v1", entries, listed, start())
}

fn catalog(ids: &[&str]) -> Catalog {
    catalog_of(&Value::Array(ids.iter().map(|id| model_entry(id)).collect()))
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

#[test]
fn an_entry_becomes_a_model_with_the_apis_limits_and_the_edit_tool() {
    let models = catalog(&[DEFAULT_MODEL]).models();

    let expected = ModelInfo::new(DEFAULT_MODEL)
        .with_context_window(1_000_000)
        .with_max_context_window(1_000_000)
        .with_max_output_tokens(128_000)
        .with_efforts(["low", "medium", "high", "xhigh", "max"], Some(DEFAULT_EFFORT))
        .with_edit_tool(EditTool::Replace);
    assert_eq!(models, vec![expected]);
    assert!(!models[0].freeform_tools);
    assert!(!models[0].prefer_websockets);
}

#[test]
fn only_active_models_are_offered_and_broken_entries_are_left_out() {
    let mut deprecated = model_entry("claude-sonnet-4-5");
    deprecated["lifecycle"] = json!("deprecated");
    let mut retired = model_entry("claude-opus-4-1");
    retired["lifecycle"] = json!("retired");
    let mut unlabelled = model_entry("claude-haiku-5-5");
    unlabelled.as_object_mut().unwrap().remove("lifecycle");
    let list = json!([
        model_entry("claude-fable-5-1"),
        deprecated,
        retired,
        {"id": ""},
        {"display_name": "no id"},
        unlabelled,
    ]);

    let catalog = catalog_of(&list);

    assert_eq!(ids(&catalog.models()), ["claude-fable-5-1", "claude-haiku-5-5"]);
    assert_eq!(catalog.left_out(), 4);
}

#[test]
fn the_efforts_are_the_supported_levels_in_their_order() {
    let mut entry = model_entry("claude-sonnet-5-5");
    entry["capabilities"]["effort"] = json!({
        "supported": true,
        "max": {"supported": true},
        "high": {"supported": true},
        "low": {"supported": false},
        "ultra": {"supported": true},
        "medium": {"supported": true},
    });
    let models = catalog_of(&json!([entry])).models();
    assert_eq!(models[0].efforts, ["medium", "high", "max", "ultra"]);
    assert_eq!(models[0].default_effort.as_deref(), Some(DEFAULT_EFFORT));

    let mut without_medium = model_entry("claude-sonnet-5-5");
    without_medium["capabilities"]["effort"] = json!({"high": {"supported": true}});
    let models = catalog_of(&json!([without_medium])).models();
    assert_eq!(models[0].efforts, ["high"]);
    assert_eq!(models[0].default_effort, None, "no default that the model does not take");

    let mut unsupported = model_entry("claude-haiku-5-5");
    unsupported["capabilities"]["effort"]["supported"] = json!(false);
    assert!(catalog_of(&json!([unsupported])).models()[0].efforts.is_empty());

    let mut bare = model_entry("claude-haiku-5-5");
    bare.as_object_mut().unwrap().remove("capabilities");
    assert!(catalog_of(&json!([bare])).models()[0].efforts.is_empty());
}

#[test]
fn a_missing_or_broken_limit_is_unknown_and_never_guessed() {
    let mut entry = model_entry("claude-next");
    entry["max_input_tokens"] = json!(0);
    entry["max_tokens"] = json!("lots");

    let models = catalog_of(&json!([entry])).models();

    assert_eq!(models[0].context_window, None);
    assert_eq!(models[0].max_context_window, None);
    assert_eq!(models[0].max_output_tokens, None);
}

#[test]
fn the_default_is_claude_codes_model_when_it_is_on_offer() {
    let listed = catalog(&["claude-fable-5-1", DEFAULT_MODEL, "claude-haiku-5-5"]);
    assert_eq!(listed.default_model().as_deref(), Some(DEFAULT_MODEL));
    let without = catalog(&["claude-sonnet-5-5", "claude-haiku-5-5"]);
    assert_eq!(without.default_model().as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(catalog(&[]).default_model(), None);

    let mut deprecated = model_entry(DEFAULT_MODEL);
    deprecated["lifecycle"] = json!("deprecated");
    let catalog = catalog_of(&json!([deprecated, model_entry("claude-haiku-5-5")]));
    assert_eq!(catalog.default_model().as_deref(), Some("claude-haiku-5-5"));
}

#[test]
fn a_shared_catalog_starts_without_a_list() {
    assert_eq!(ModelCatalog::new().current(), None);
}

#[test]
fn a_list_with_models_becomes_current_and_one_without_is_refused() {
    let shared = ModelCatalog::new();
    let applied = shared.apply(catalog(&[DEFAULT_MODEL]));
    let Applied::Changed(current) = applied else { panic!("{applied:?}") };
    assert!(Arc::ptr_eq(&current, &shared.current().unwrap()));

    let mut deprecated = model_entry("claude-sonnet-4-5");
    deprecated["lifecycle"] = json!("deprecated");
    assert_eq!(
        shared.apply(catalog_of(&json!([deprecated, {"id": 1}]))),
        Applied::Refused { listed: 2 }
    );
    assert_eq!(ids(&shared.current().unwrap().models()), [DEFAULT_MODEL]);
}

#[test]
fn a_catalog_from_the_cache_is_current_from_the_start() {
    let mut cached = catalog(&[DEFAULT_MODEL]);
    cached.origin = CatalogOrigin::Cache;
    let shared = ModelCatalog::with_catalog(cached);
    assert_eq!(shared.current().unwrap().origin(), CatalogOrigin::Cache);
}

#[test]
fn the_config_lowers_a_limit_up_to_the_catalogs_and_adds_its_own_models() {
    let catalog = catalog(&[DEFAULT_MODEL, "claude-haiku-5-5"]).models();
    let extra = [
        ModelInfo::new(DEFAULT_MODEL).with_context_window(400_000).with_max_output_tokens(64_000),
        ModelInfo::new("claude-haiku-5-5")
            .with_context_window(4_000_000)
            .with_max_output_tokens(500_000),
        ModelInfo::new("claude-private-1").with_context_window(200_000),
    ];

    let models = with_extra(catalog, &extra);

    assert_eq!(ids(&models), [DEFAULT_MODEL, "claude-haiku-5-5", "claude-private-1"]);
    assert_eq!(models[0].context_window, Some(400_000));
    assert_eq!(models[0].max_output_tokens, Some(64_000));
    assert_eq!(models[1].context_window, Some(1_000_000), "never above the API's window");
    assert_eq!(models[1].max_output_tokens, Some(128_000), "never above the API's cap");
    assert_eq!(models[2].context_window, Some(200_000));
    assert_eq!(models[2].edit_tool, EditTool::Replace);
}
