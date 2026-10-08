use std::os::unix::fs::PermissionsExt as _;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{read_cache, write_cache};
use crate::OpenAiError;
use crate::catalog::{Catalog, CatalogOrigin, entries_of};
use crate::config::Backend;
use crate::testing::{catalog_fixture, start};

const BASE_URL: &str = "https://backend.test/codex";

fn fetched() -> Catalog {
    let body: Value = serde_json::from_str(&catalog_fixture("models.json")).unwrap();
    let (entries, _) = entries_of(&body).unwrap();
    Catalog::from_backend(
        Backend::Subscription,
        BASE_URL,
        entries,
        Some("\"etag-1\"".to_owned()),
        start(),
    )
}

#[test]
fn a_written_catalog_reads_back_as_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("model_catalog.json");
    let catalog = fetched();
    write_cache(&path, &catalog).unwrap();

    let read = read_cache(&path, Backend::Subscription, BASE_URL).unwrap().unwrap();
    assert_eq!(read.origin(), CatalogOrigin::Cache);
    assert_eq!(read.fetched_at(), Some(start()));
    assert_eq!(read.etag(), Some("\"etag-1\""));
    assert_eq!(read.models(), catalog.models());
    assert_eq!(read.left_out(), catalog.left_out(), "the hidden entries stay in the file");
    assert_eq!(read.etag_for(BASE_URL), Some("\"etag-1\""));
}

#[test]
fn the_file_and_its_directory_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let path = state.join("model_catalog.json");
    write_cache(&path, &fetched()).unwrap();
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode(&state) & 0o777, 0o700);
    assert_eq!(mode(&path) & 0o777, 0o600);
}

#[test]
fn no_file_is_no_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model_catalog.json");
    assert_eq!(read_cache(&path, Backend::Subscription, BASE_URL).unwrap(), None);
}

#[test]
fn the_builtin_table_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model_catalog.json");
    write_cache(&path, &Catalog::builtin(Backend::Subscription)).unwrap();
    assert!(!path.exists());
}

#[test]
fn a_cache_of_another_backend_or_version_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model_catalog.json");
    write_cache(&path, &fetched()).unwrap();
    assert_eq!(read_cache(&path, Backend::Subscription, "https://other.test/codex").unwrap(), None);

    let mut file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file["version"] = json!(2);
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    assert_eq!(read_cache(&path, Backend::Subscription, BASE_URL).unwrap(), None);
}

#[test]
fn a_file_that_is_not_a_cache_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model_catalog.json");
    std::fs::write(&path, "{ not json").unwrap();
    let error = read_cache(&path, Backend::Subscription, BASE_URL).unwrap_err();
    assert!(matches!(error, OpenAiError::CacheParse { .. }), "{error:?}");
    std::fs::write(&path, r#"{"version": 1}"#).unwrap();
    let error = read_cache(&path, Backend::Subscription, BASE_URL).unwrap_err();
    assert!(matches!(error, OpenAiError::CacheParse { .. }), "{error:?}");
}
