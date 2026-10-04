use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::document;
use crate::fixtures_check::{event_samples, method_samples};
use crate::{Event, ScopeName};

/// Every `$ref` in `value`.
fn refs(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, member) in object {
                match (key.as_str(), member) {
                    ("$ref", Value::String(target)) => {
                        found.insert(target.clone());
                    }
                    _ => refs(member, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| refs(item, found)),
        _ => {}
    }
}

#[test]
fn the_method_table_matches_the_method_enum() {
    let doc = document();
    let listed: Vec<(String, Value, bool)> = doc["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().unwrap().to_owned(),
                entry["scope"].clone(),
                entry["stream"].as_bool().unwrap(),
            )
        })
        .collect();
    let expected: Vec<(String, Value, bool)> = method_samples()
        .iter()
        .map(|method| {
            (method.name().to_owned(), json!(ScopeName::for_method(method)), method.is_stream())
        })
        .collect();
    assert_eq!(listed, expected);
}

#[test]
fn every_method_has_params_and_one_answer_schema() {
    for entry in document()["methods"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        assert!(entry["params"].is_object(), "{name}");
        let answer = if entry["stream"] == json!(true) { "item" } else { "result" };
        assert!(entry[answer].is_object(), "{name} has no {answer}");
    }
}

#[test]
fn every_ref_points_into_the_definitions() {
    let doc = document();
    let mut found = BTreeSet::new();
    refs(&doc, &mut found);
    assert!(!found.is_empty());
    for target in found {
        let name = target.strip_prefix("#/$defs/").unwrap_or_else(|| panic!("{target}"));
        assert!(doc["$defs"].get(name).is_some(), "{target} is not defined");
    }
}

#[test]
fn the_event_schema_lists_every_known_kind() {
    let doc = document();
    let text = serde_json::to_string(&doc["$defs"]["Event"]).unwrap();
    for event in event_samples() {
        if !matches!(event, Event::Unknown { .. }) {
            assert!(text.contains(&format!("\"{}\"", event.kind())), "{}", event.kind());
        }
    }
}

#[test]
fn the_document_names_the_protocol_version_and_closed_sets() {
    let doc = document();
    assert_eq!(doc["protocol"], json!(crate::PROTOCOL_VERSION));
    assert_eq!(doc["error_codes"].as_array().unwrap().len(), 10);
    assert_eq!(doc["scopes"], json!(["read", "operate", "approve", "terminal", "admin"]));
}
