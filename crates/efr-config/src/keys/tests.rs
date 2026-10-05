use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use serde_json::Value as Json;

use crate::{Kind, RESTART_KEYS, SCHEMA_URL, json_schema, keys, kind};

/// Every dotted key the JSON schema describes: the top-level properties that are not
/// tables, and the properties of each table.
fn schema_keys() -> BTreeSet<String> {
    let schema = json_schema();
    let mut found = BTreeSet::new();
    let properties = schema["properties"].as_object().unwrap();
    for (name, node) in properties {
        let table = node
            .get("$ref")
            .and_then(Json::as_str)
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .and_then(|name| schema["$defs"].get(name))
            .filter(|definition| definition.get("properties").is_some());
        match table {
            Some(definition) => {
                for key in definition["properties"].as_object().unwrap().keys() {
                    found.insert(format!("{name}.{key}"));
                }
            }
            None => {
                found.insert(name.clone());
            }
        }
    }
    found
}

#[test]
fn the_keys_start_with_the_top_level_ones_in_the_order_of_the_file() {
    let keys = keys();

    assert_eq!(&keys[..4], ["log", "screen", "model.provider", "model.name"]);
    assert!(keys.contains(&"permissions.rules".to_owned()));
    assert!(keys.contains(&"shell.sudo_cache".to_owned()));
    assert_eq!(keys.last().map(String::as_str), Some("render.theme"));
}

#[test]
fn the_schema_covers_every_key_and_nothing_else() {
    let listed: BTreeSet<String> = keys().into_iter().collect();

    assert_eq!(listed, schema_keys());
    assert_eq!(keys().len(), listed.len(), "no key is listed twice");
}

#[test]
fn every_table_of_the_schema_refuses_unknown_keys() {
    let schema = json_schema();

    assert_eq!(schema["additionalProperties"], Json::Bool(false));
    for (name, definition) in schema["$defs"].as_object().unwrap() {
        if definition.get("properties").is_some() {
            assert_eq!(definition["additionalProperties"], Json::Bool(false), "{name}");
        }
    }
    assert_eq!(schema["$id"], Json::String(SCHEMA_URL.to_owned()));
}

#[test]
fn every_key_has_a_description_in_the_schema() {
    let schema = json_schema();
    for key in keys() {
        let (table, name) = key.split_once('.').map_or((None, key.as_str()), |(t, n)| (Some(t), n));
        let node = match table {
            None => &schema["properties"][name],
            Some(table) => {
                let reference = schema["properties"][table]["$ref"].as_str().unwrap();
                let definition = reference.strip_prefix("#/$defs/").unwrap();
                &schema["$defs"][definition]["properties"][name]
            }
        };
        assert!(node.get("description").is_some(), "{key} has no description");
    }
}

#[test]
fn each_key_has_a_kind() {
    assert_eq!(kind("log"), Some(Kind::String));
    assert_eq!(
        kind("screen"),
        Some(Kind::Choice(vec!["auto".to_owned(), "vt100".to_owned(), "ghostty".to_owned()]))
    );
    assert_eq!(
        kind("permissions.mode"),
        Some(Kind::Choice(vec!["manual".to_owned(), "cautious".to_owned(), "auto".to_owned()]))
    );
    assert_eq!(
        kind("shell.sudo_cache"),
        Some(Kind::Choice(vec!["keep".to_owned(), "per_call".to_owned()]))
    );
    assert_eq!(kind("model.name"), Some(Kind::String));
    assert_eq!(kind("model.max_output_tokens"), Some(Kind::Integer));
    assert_eq!(kind("shell.login"), Some(Kind::Boolean));
    assert_eq!(kind("shell.program"), Some(Kind::String));
    assert_eq!(kind("openai.models"), Some(Kind::List));
    assert_eq!(kind("permissions.secret_paths"), Some(Kind::List));
    assert_eq!(kind("permissions.rules"), Some(Kind::Rules));
    assert_eq!(kind("shell.colour"), None);
    assert_eq!(kind("model"), None);
    for key in keys() {
        assert!(kind(&key).is_some(), "{key} has no kind");
    }
}

#[test]
fn a_kind_says_what_its_values_are() {
    assert_eq!(
        Kind::Choice(vec!["keep".to_owned(), "per_call".to_owned()]).to_string(),
        "keep or per_call"
    );
    assert_eq!(Kind::Integer.to_string(), "a whole number");
}

#[test]
fn the_restart_keys_are_keys() {
    let keys = keys();
    for key in RESTART_KEYS {
        assert!(keys.iter().any(|known| known == key), "{key}");
    }
}
