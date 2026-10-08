use efr_provider::ModelInfo;
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::{
    Applied, CLIENT_VERSION, Catalog, CatalogOrigin, Fetched, ModelCatalog, entries_of,
    version_reaches, with_extra,
};
use crate::config::Backend;
use crate::testing::{catalog_fixture, start};

const BASE_URL: &str = "https://backend.test/codex";

fn fixture_catalog(backend: Backend, etag: Option<&str>, now: Timestamp) -> Catalog {
    let body: Value = serde_json::from_str(&catalog_fixture("models.json")).unwrap();
    let (entries, _) = entries_of(&body).unwrap();
    Catalog::from_backend(backend, BASE_URL, entries, etag.map(str::to_owned), now)
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

fn model<'a>(models: &'a [ModelInfo], id: &str) -> &'a ModelInfo {
    models.iter().find(|model| model.id == id).unwrap()
}

#[test]
fn every_entry_that_reads_is_kept_and_the_broken_ones_are_counted() {
    let body: Value = serde_json::from_str(&catalog_fixture("models.json")).unwrap();
    let (entries, broken) = entries_of(&body).unwrap();
    let slugs: Vec<&str> = entries.iter().map(|entry| entry.slug.as_str()).collect();
    assert_eq!(
        slugs,
        [
            "gpt-7-sol",
            "gpt-7-luna",
            "gpt-7-hidden",
            "gpt-7-unpicked",
            "gpt-7-too-new",
            "gpt-7-odd-window"
        ]
    );
    assert_eq!(broken, 2, "the entry without a slug and the one with a bad member");
}

#[test]
fn a_body_without_a_list_of_models_has_no_entries() {
    assert_eq!(entries_of(&json!({"data": []})), None);
    assert_eq!(entries_of(&json!({"models": {}})), None);
    assert_eq!(entries_of(&json!({"models": []})).map(|(entries, _)| entries.len()), Some(0));
}

#[test]
fn the_listed_models_are_offered_best_priority_first() {
    let catalog = fixture_catalog(Backend::Subscription, None, start());
    let models = catalog.models();
    assert_eq!(ids(&models), ["gpt-7-luna", "gpt-7-sol", "gpt-7-odd-window"]);
    assert_eq!(catalog.default_model().as_deref(), Some("gpt-7-luna"));
    assert_eq!(catalog.left_out(), 3);
}

#[test]
fn hidden_and_too_new_models_are_left_out() {
    let models = fixture_catalog(Backend::Subscription, None, start()).models();
    for id in ["gpt-7-hidden", "gpt-7-unpicked", "gpt-7-too-new"] {
        assert!(!ids(&models).contains(&id), "{id}");
    }
}

#[test]
fn the_api_backend_leaves_out_the_models_that_it_does_not_serve() {
    let models = fixture_catalog(Backend::Api, None, start()).models();
    assert_eq!(ids(&models), ["gpt-7-sol", "gpt-7-odd-window"]);
}

#[test]
fn each_model_gets_its_window_efforts_and_tool_form() {
    let models = fixture_catalog(Backend::Subscription, None, start()).models();
    let sol = model(&models, "gpt-7-sol");
    assert_eq!(sol.context_window, Some(300_000));
    assert_eq!(sol.max_context_window, Some(900_000));
    assert_eq!(sol.efforts, ["low", "medium", "high"]);
    assert_eq!(sol.default_effort.as_deref(), Some("medium"));
    assert!(sol.freeform_tools);
    assert!(sol.prefer_websockets);
    assert_eq!(sol.max_output_tokens, None, "a catalog has no output limit");

    let luna = model(&models, "gpt-7-luna");
    assert_eq!(luna.context_window, Some(128_000));
    assert_eq!(luna.max_context_window, Some(128_000), "no larger window than its own");
    assert!(!luna.freeform_tools, "no apply_patch_tool_type is the function form");
    assert!(!luna.prefer_websockets);

    let odd = model(&models, "gpt-7-odd-window");
    assert_eq!(odd.context_window, Some(500_000), "a broken window takes the largest one");
    assert_eq!(odd.max_context_window, Some(500_000));
}

#[rstest]
#[case("0.0.2", None, true)]
#[case("0.0.2", Some("0.0.1"), true)]
#[case("0.0.2", Some("0.0.2"), true)]
#[case("0.0.2", Some("0.0.3"), false)]
#[case("0.0.2", Some("0.153.0"), false)]
#[case("1.2.0", Some("1.10.0"), false)]
#[case("1.10.0", Some("1.2.0"), true)]
#[case("1.2.3-dev", Some("1.2.3"), true)]
#[case("2", Some("1.9.9"), true)]
#[case("0.0.2", Some("not a version"), true)]
#[case("0.0.2", Some("1.2.3.4"), true)]
fn a_minimal_client_version_is_compared_part_by_part(
    #[case] client: &str,
    #[case] minimal: Option<&str>,
    #[case] reaches: bool,
) {
    assert_eq!(version_reaches(client, minimal), reaches, "{client} against {minimal:?}");
}

#[test]
fn efr_sends_its_own_version() {
    assert_eq!(CLIENT_VERSION, env!("CARGO_PKG_VERSION"));
}

#[test]
fn only_a_list_from_the_same_backend_for_this_efr_sends_its_tag() {
    let fetched = fixture_catalog(Backend::Subscription, Some("\"v1\""), start());
    assert_eq!(fetched.etag_for(BASE_URL), Some("\"v1\""));
    assert_eq!(fetched.etag_for("https://other.test/codex"), None);
    let older = Catalog { client_version: Some("0.0.0-old".to_owned()), ..fetched.clone() };
    assert_eq!(older.etag_for(BASE_URL), None);
    assert_eq!(Catalog::builtin(Backend::Subscription).etag_for(BASE_URL), None);
}

#[test]
fn a_new_list_replaces_the_current_one() {
    let shared = ModelCatalog::new(Catalog::builtin(Backend::Subscription));
    let fetched = fixture_catalog(Backend::Subscription, Some("\"v1\""), start());
    let applied = shared.apply(Fetched::Changed(fetched.clone()), start());
    assert_eq!(applied, Applied::Changed(std::sync::Arc::new(fetched)));
    let current = shared.current();
    assert_eq!(current.origin(), CatalogOrigin::Backend);
    assert_eq!(current.fetched_at(), Some(start()));
    assert_eq!(current.default_model().as_deref(), Some("gpt-7-luna"));
}

#[test]
fn not_modified_confirms_the_current_list_with_a_new_time() {
    let cached = Catalog {
        origin: CatalogOrigin::Cache,
        ..fixture_catalog(Backend::Subscription, Some("\"v1\""), start())
    };
    let shared = ModelCatalog::new(cached);
    let later = start().checked_add(SignedDuration::from_hours(2)).unwrap();
    let Applied::Revalidated(current) = shared.apply(Fetched::NotModified, later) else {
        panic!("not revalidated");
    };
    assert_eq!(current.origin(), CatalogOrigin::Backend);
    assert_eq!(current.fetched_at(), Some(later));
    assert_eq!(current.etag(), Some("\"v1\""));
    assert_eq!(shared.current(), current);
}

#[test]
fn not_modified_without_a_list_changes_nothing() {
    let shared = ModelCatalog::new(Catalog::builtin(Backend::Subscription));
    assert_eq!(shared.apply(Fetched::NotModified, start()), Applied::Ignored);
    assert_eq!(shared.current().origin(), CatalogOrigin::Builtin);
}

#[test]
fn a_list_that_offers_no_model_to_this_efr_is_refused() {
    let shared = ModelCatalog::new(Catalog::builtin(Backend::Subscription));
    let body = json!({"models": [
        {"slug": "gpt-8", "visibility": "list", "minimal_client_version": "999.0.0"},
        {"slug": "gpt-8-hidden", "visibility": "hide"},
    ]});
    let (entries, _) = entries_of(&body).unwrap();
    let fetched = Catalog::from_backend(Backend::Subscription, BASE_URL, entries, None, start());
    assert_eq!(shared.apply(Fetched::Changed(fetched), start()), Applied::Refused { listed: 2 });
    assert_eq!(shared.current().origin(), CatalogOrigin::Builtin);
}

#[test]
fn the_models_of_the_config_lie_over_the_catalog() {
    let catalog = vec![
        ModelInfo::new("a").with_context_window(100_000).with_max_output_tokens(8_000),
        ModelInfo::new("b").with_context_window(200_000),
    ];
    let extra = vec![
        ModelInfo::new("a").with_max_output_tokens(4_000),
        ModelInfo::new("c").with_context_window(50_000),
    ];
    let merged = with_extra(catalog, &extra);
    assert_eq!(ids(&merged), ["a", "b", "c"]);
    assert_eq!(merged[0].context_window, Some(100_000), "a missing value keeps the catalog's");
    assert_eq!(merged[0].max_output_tokens, Some(4_000));
    assert_eq!(merged[2].context_window, Some(50_000));
}
