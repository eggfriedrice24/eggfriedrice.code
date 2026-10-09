use std::os::unix::fs::PermissionsExt as _;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{read_cache, write_cache};
use crate::AnthropicError;
use crate::catalog::{Catalog, CatalogOrigin, entries_of};
use crate::testing::{model_entry, start};

const BASE_URL: &str = "https://api.anthropic.test/v1";

fn fetched() -> Catalog {
    let mut retired = model_entry("claude-sonnet-4-5");
    retired["lifecycle"] = json!("deprecated");
    let list = json!([model_entry("claude-opus-5-5"), retired, {"id": 7}]);
    let (entries, broken) = entries_of(&list);
    Catalog::from_backend(BASE_URL, entries, 2 + broken, start())
}

#[test]
fn a_written_catalog_reads_back_as_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("anthropic_model_catalog.json");
    let catalog = fetched();
    write_cache(&path, &catalog).unwrap();

    let read = read_cache(&path, BASE_URL).unwrap().unwrap();
    assert_eq!(read.origin(), CatalogOrigin::Cache);
    assert_eq!(read.fetched_at(), start());
    assert_eq!(read.models(), catalog.models());
    assert_eq!(read.default_model(), catalog.default_model());
    assert_eq!(read.left_out(), 1, "the deprecated entry stays in the file, the broken one not");
}

#[test]
fn the_file_keeps_the_apis_own_form() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anthropic_model_catalog.json");
    write_cache(&path, &fetched()).unwrap();

    let file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(file["version"], json!(1));
    assert_eq!(file["base_url"], json!(BASE_URL));
    assert_eq!(file["fetched_at"], json!("2026-10-04T12:00:00Z"));
    let first = &file["models"][0];
    assert_eq!(first["id"], json!("claude-opus-5-5"));
    assert_eq!(first["max_input_tokens"], json!(1_000_000));
    assert_eq!(first["capabilities"], model_entry("claude-opus-5-5")["capabilities"]);
    assert_eq!(first["lifecycle"], json!("active"));
}

#[test]
fn the_file_and_its_directory_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let path = state.join("anthropic_model_catalog.json");
    write_cache(&path, &fetched()).unwrap();
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode(&state) & 0o777, 0o700);
    assert_eq!(mode(&path) & 0o777, 0o600);
}

#[test]
fn no_file_is_no_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anthropic_model_catalog.json");
    assert_eq!(read_cache(&path, BASE_URL).unwrap(), None);
}

#[test]
fn a_cache_of_another_base_url_or_version_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anthropic_model_catalog.json");
    write_cache(&path, &fetched()).unwrap();
    assert_eq!(read_cache(&path, "https://proxy.test/v1").unwrap(), None);
    assert!(read_cache(&path, &format!("{BASE_URL}/")).unwrap().is_some(), "a trailing slash");

    let mut file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file["version"] = json!(2);
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    assert_eq!(read_cache(&path, BASE_URL).unwrap(), None);
}

#[test]
fn a_file_that_is_not_a_catalog_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anthropic_model_catalog.json");
    std::fs::write(&path, "not json").unwrap();
    let error = read_cache(&path, BASE_URL).unwrap_err();
    assert!(matches!(error, AnthropicError::CacheParse { .. }), "{error:?}");

    std::fs::write(&path, r#"{"version": 1}"#).unwrap();
    let error = read_cache(&path, BASE_URL).unwrap_err();
    assert!(matches!(error, AnthropicError::CacheParse { .. }), "{error:?}");
}

#[test]
fn a_directory_in_the_way_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = read_cache(dir.path(), BASE_URL).unwrap_err();
    assert!(matches!(error, AnthropicError::CacheRead { .. }), "{error:?}");
}
