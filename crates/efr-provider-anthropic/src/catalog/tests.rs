use std::sync::Arc;

use efr_provider::{EditTool, ModelInfo};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::{Applied, Catalog, CatalogOrigin, ModelCatalog};
use crate::DEFAULT_MODEL;

fn claude(id: &str) -> ModelInfo {
    ModelInfo::new(id).with_edit_tool(EditTool::Replace)
}

fn catalog(ids: &[&str], listed: usize) -> Catalog {
    Catalog {
        models: ids.iter().map(|id| claude(id)).collect(),
        listed,
        origin: CatalogOrigin::Backend,
        fetched_at: Timestamp::UNIX_EPOCH,
    }
}

#[test]
fn the_default_is_claude_codes_model_when_it_is_on_offer() {
    let listed = catalog(&["claude-fable-5-1", DEFAULT_MODEL, "claude-haiku-5-5"], 4);
    assert_eq!(listed.default_model().as_deref(), Some(DEFAULT_MODEL));
    assert_eq!(listed.left_out(), 1);
    let without = catalog(&["claude-sonnet-5-5", "claude-haiku-5-5"], 2);
    assert_eq!(without.default_model().as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(catalog(&[], 3).default_model(), None);
}

#[test]
fn a_shared_catalog_starts_without_a_list() {
    assert_eq!(ModelCatalog::new().current(), None);
}

#[test]
fn a_list_with_models_becomes_current_and_an_empty_one_is_refused() {
    let shared = ModelCatalog::new();
    let applied = shared.apply(catalog(&[DEFAULT_MODEL], 1));
    let Applied::Changed(current) = applied else { panic!("{applied:?}") };
    assert!(Arc::ptr_eq(&current, &shared.current().unwrap()));

    assert_eq!(shared.apply(catalog(&[], 2)), Applied::Refused { listed: 2 });
    assert_eq!(shared.current().unwrap().models(), vec![claude(DEFAULT_MODEL)]);
}

#[test]
fn a_catalog_from_the_cache_is_current_from_the_start() {
    let mut cached = catalog(&[DEFAULT_MODEL], 1);
    cached.origin = CatalogOrigin::Cache;
    let shared = ModelCatalog::with_catalog(cached);
    assert_eq!(shared.current().unwrap().origin(), CatalogOrigin::Cache);
}
