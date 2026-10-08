use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use serde_json::Value as Json;

use crate::{COLOR_ROLES, Kind, RESTART_KEYS, SCHEMA_URL, description, json_schema, keys, kind};

/// Every dotted key the JSON schema describes: the properties that are not tables, at
/// the top level and in each table, however deep.
fn schema_keys() -> BTreeSet<String> {
    let schema = json_schema();
    let mut found = BTreeSet::new();
    add_schema_keys(&schema, &schema, None, &mut found);
    found
}

fn add_schema_keys(schema: &Json, node: &Json, prefix: Option<&str>, found: &mut BTreeSet<String>) {
    for (name, property) in node["properties"].as_object().unwrap() {
        let key = prefix.map_or_else(|| name.clone(), |prefix| format!("{prefix}.{name}"));
        let table = property
            .get("$ref")
            .and_then(Json::as_str)
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .and_then(|name| schema["$defs"].get(name))
            .filter(|definition| definition.get("properties").is_some());
        match table {
            Some(definition) => add_schema_keys(schema, definition, Some(&key), found),
            None => {
                found.insert(key);
            }
        }
    }
}

#[test]
fn the_keys_start_with_the_top_level_ones_in_the_order_of_the_file() {
    let keys = keys();

    assert_eq!(&keys[..4], ["log", "screen", "model.provider", "model.name"]);
    assert!(keys.contains(&"permissions.rules".to_owned()));
    assert!(keys.contains(&"shell.sudo_cache".to_owned()));
    assert_eq!(keys.last().map(String::as_str), Some("render.colors.diff.hunk"));
}

#[test]
fn the_render_keys_hold_one_colour_key_per_role() {
    let render: Vec<String> = keys().into_iter().filter(|key| key.starts_with("render.")).collect();
    let mut expected: Vec<String> = [
        "theme",
        "theme_dark",
        "theme_light",
        "palette",
        "motion",
        "turn_summary",
        "diff_lines",
        "progress",
        "turn_input",
    ]
    .map(|key| format!("render.{key}"))
    .into();
    expected.extend(COLOR_ROLES.iter().map(|role| format!("render.colors.{role}")));
    assert_eq!(render, expected);
    for role in COLOR_ROLES {
        assert_eq!(kind(&format!("render.colors.{role}")), Some(Kind::String), "{role}");
    }
    assert_eq!(kind("render.colors"), None);
    assert_eq!(kind("render.palette"), Some(Kind::String));
    assert_eq!(kind("render.motion"), Some(Kind::Boolean));
    assert_eq!(kind("render.turn_summary"), Some(Kind::Boolean));
    assert_eq!(kind("render.turn_input"), Some(Kind::Boolean));
    let choices = ["auto", "on", "off"].map(str::to_owned).to_vec();
    assert_eq!(kind("render.progress"), Some(Kind::Choice(choices)));
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
    for key in keys() {
        assert!(description(&key).is_some_and(|text| text.len() > 10), "{key} has no description");
    }
    assert_eq!(
        description("render.colors.diff.add").as_deref(),
        Some("Added lines. Unset: slot 2 (green).")
    );
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
