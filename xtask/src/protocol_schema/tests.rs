use std::path::Path;

use serde_json::json;

use super::{SCHEMA, generated, run, tuples};

#[test]
fn a_check_of_a_tree_without_the_file_fails_and_writing_makes_it_current() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();

    assert!(!run(root.path(), true).unwrap());
    assert!(run(root.path(), false).unwrap());
    assert!(run(root.path(), true).unwrap());
    assert!(root.path().join(SCHEMA).is_file());
}

#[test]
fn a_check_of_a_stale_file_fails_and_leaves_it_as_it_was() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join(SCHEMA), "{}\n").unwrap();

    assert!(!run(root.path(), true).unwrap());
    assert_eq!(std::fs::read_to_string(root.path().join(SCHEMA)).unwrap(), "{}\n");
}

#[test]
fn the_committed_file_is_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let on_disk = std::fs::read_to_string(root.join(SCHEMA)).unwrap();
    assert!(
        on_disk == generated().unwrap(),
        "{SCHEMA} is not current; run `cargo xtask protocol-schema`"
    );
}

#[test]
fn the_protocol_has_no_tuple() {
    assert_eq!(tuples(&efr_protocol::schema::codegen_document()), Vec::<String>::new());
}

#[test]
fn a_tuple_is_found_in_either_form_and_an_array_of_one_type_is_not() {
    let schema = json!({
        "$defs": {
            "Pair": { "type": "array", "prefixItems": [{ "type": "string" }, { "type": "integer" }] },
            "Old": { "type": "array", "items": [{ "type": "string" }] },
            "List": { "type": "array", "items": { "type": "string" } },
            "Nested": { "oneOf": [{ "type": "array", "prefixItems": [] }] },
        },
    });
    assert_eq!(
        tuples(&schema),
        ["/$defs/Nested/oneOf/0/prefixItems", "/$defs/Old/items", "/$defs/Pair/prefixItems"]
    );
}

#[test]
fn the_file_is_a_generator_friendly_root_with_definitions() {
    let document: serde_json::Value = serde_json::from_str(&generated().unwrap()).unwrap();
    assert_eq!(document["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert_eq!(document["title"], "EfrProtocol");
    for top in ["ClientFrame", "ServerFrame", "MethodParams", "MethodResults", "Event"] {
        assert!(document["$defs"][top].is_object(), "{top}");
    }
}
