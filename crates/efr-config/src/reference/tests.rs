use pretty_assertions::assert_eq;

use crate::{Applies, RESTART_KEYS, json_schema, keys, reference, schema_text};

#[test]
fn the_reference_has_one_row_per_key_in_the_order_of_the_file() {
    let text = reference().unwrap();
    let rows: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| line.split('`').nth(1).unwrap())
        .collect();
    let names: Vec<String> = keys()
        .iter()
        .map(|key| key.split_once('.').map_or(key.as_str(), |(_, name)| name).to_owned())
        .collect();
    assert_eq!(rows, names);
}

#[test]
fn the_colour_keys_are_rows_of_the_render_table() {
    let text = reference().unwrap();
    let render = text.split("\n## [render]\n").nth(1).unwrap();
    assert!(render.contains("| `palette` | unset | client |"), "{render}");
    assert!(render.contains("| `colors.accent` | unset | client |"), "{render}");
    assert!(render.contains("| `colors.diff.hunk` | unset | client |"), "{render}");
}

#[test]
fn the_reference_says_when_each_key_applies_and_gives_its_default() {
    let text = reference().unwrap();
    assert!(text.contains("| `screen` | `\"auto\"` | restart |"), "{text}");
    assert!(text.contains("| `idle_minutes` | `60` | live |"), "{text}");
    assert!(text.contains("| `name` | unset | live |"), "{text}");
    assert!(text.contains("| `system_prompt` | built in | live |"), "{text}");
    assert!(text.contains("| `theme` | unset | client |"), "{text}");
    assert!(text.contains("\n## [permissions]\n\nThe permission mode"), "{text}");
    let restart_rows = text.lines().filter(|line| line.contains(" | restart | ")).count();
    assert_eq!(restart_rows, RESTART_KEYS.len());
}

#[test]
fn every_row_is_one_line_with_four_cells() {
    for line in reference().unwrap().lines().filter(|line| line.starts_with("| `")) {
        let cells = line.replace("\\|", "").matches('|').count();
        assert_eq!(cells, 5, "{line}");
    }
}

#[test]
fn applies_follows_the_restart_keys_and_the_client_table() {
    assert_eq!(Applies::of("screen"), Applies::Restart);
    assert_eq!(Applies::of("openai.api_base_url"), Applies::Restart);
    assert_eq!(Applies::of("log"), Applies::Live);
    assert_eq!(Applies::of("permissions.rules"), Applies::Live);
    assert_eq!(Applies::of("render.theme"), Applies::Client);
}

#[test]
fn the_schema_text_is_the_schema() {
    let text = schema_text();
    assert!(text.ends_with("}\n"));
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed, json_schema());
}

/// `docs/config.schema.json` offers every mode for `permissions.mode`, and its `full`
/// says that the file needs the gate key with it.
#[test]
fn the_schema_offers_every_mode_and_full_names_its_gate() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/config.schema.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mode = &schema["$defs"]["PermissionSettings"]["properties"]["mode"];
    assert_eq!(mode["$ref"], "#/$defs/ConfigMode", "{mode}");
    let values = schema["$defs"]["ConfigMode"]["oneOf"].as_array().unwrap();
    let names: Vec<&str> = values.iter().map(|value| value["const"].as_str().unwrap()).collect();
    let known: Vec<&str> = efr_protocol::Mode::ALL.iter().map(|mode| mode.as_str()).collect();
    assert_eq!(names, known);
    let full = values[3]["description"].as_str().unwrap();
    assert!(full.contains("`permissions.allow_full = true`"), "{full}");
}
