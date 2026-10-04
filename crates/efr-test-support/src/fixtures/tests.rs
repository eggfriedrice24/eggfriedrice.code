use std::path::Path;

use pretty_assertions::assert_eq;

use super::{crate_dir, dir, path};
use crate::{TestSupportError, Transcript};

fn this_crate() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn file_macro_paths_resolve_to_this_crate() {
    assert_eq!(crate_dir(file!()).unwrap(), this_crate());
    assert_eq!(dir(file!()).unwrap(), this_crate().join("fixtures"));
}

#[test]
fn a_fixture_path_is_under_the_fixtures_directory() {
    let found = path(file!(), "transcripts/single_exchange.ndjson").unwrap();
    assert_eq!(found, this_crate().join("fixtures/transcripts/single_exchange.ndjson"));
    assert!(found.is_file(), "{} is missing", found.display());
}

#[test]
fn a_fixture_that_does_not_exist_yet_still_has_a_path() {
    let found = path(file!(), "new/case.ndjson").unwrap();
    assert_eq!(found, this_crate().join("fixtures/new/case.ndjson"));
}

#[test]
fn an_absolute_source_path_works_too() {
    let absolute = this_crate().join("src/fixtures.rs");
    assert_eq!(crate_dir(absolute.to_str().unwrap()).unwrap(), this_crate());
}

#[test]
fn a_source_file_that_does_not_exist_is_an_error() {
    let err = path("crates/efr-nothing/src/lib.rs", "x").unwrap_err();
    assert!(
        matches!(&err, TestSupportError::NoCrateForSource { source_file }
            if source_file.as_path() == Path::new("crates/efr-nothing/src/lib.rs")),
        "{err:?}"
    );
}

#[test]
fn the_fixture_transcript_reads_through_its_path() {
    let transcript =
        Transcript::read(path(file!(), "transcripts/single_exchange.ndjson").unwrap()).unwrap();
    let kinds: Vec<&str> = transcript.records().map(|record| record.kind()).collect();
    assert_eq!(
        kinds,
        ["client_frame", "provider_request", "provider_sse", "provider_sse", "event"]
    );
}
