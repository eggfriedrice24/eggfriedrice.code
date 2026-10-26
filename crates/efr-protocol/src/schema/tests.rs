use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{codegen_document, codegen_parts, document};
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

#[test]
fn the_last_command_is_a_member_of_prompt_send_and_never_of_a_shell_context() {
    let doc = document();
    let context = &doc["$defs"]["ShellContext"]["properties"];
    assert!(context.is_object(), "{doc:#}");
    assert!(context.get("last_command").is_none(), "{context:#}");
    assert!(context.get("recent_commands").is_none(), "{context:#}");
    let prompt_send = doc["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == json!("prompt.send"))
        .unwrap();
    assert_eq!(prompt_send["params"], json!({ "$ref": "#/$defs/PromptSend" }));
    let params = &doc["$defs"]["PromptSend"]["properties"];
    assert!(params["last_command"].is_object(), "{params:#}");
    assert!(params["recent_commands"].is_object(), "{params:#}");
}

#[test]
fn no_top_type_of_the_codegen_file_hides_a_wire_type() {
    let (definitions, tops) = codegen_parts();
    assert!(!tops.is_empty());
    for name in tops.keys() {
        assert!(!definitions.contains_key(name), "{name} is a wire type already");
    }
}

#[test]
fn every_ref_of_the_codegen_file_points_into_its_definitions() {
    let doc = codegen_document();
    let mut found = BTreeSet::new();
    refs(&doc, &mut found);
    assert!(!found.is_empty());
    for target in found {
        let name = target.strip_prefix("#/$defs/").unwrap_or_else(|| panic!("{target}"));
        assert!(doc["$defs"].get(name).is_some(), "{target} is not defined");
    }
}

#[test]
fn the_codegen_file_maps_every_method_to_its_params_and_answer() {
    let doc = codegen_document();
    let reference = document();
    let defs = &doc["$defs"];
    let names: Vec<&str> = method_samples().iter().map(|method| method.name()).collect();
    assert_eq!(defs["MethodName"]["enum"], json!(names));
    let streams: Vec<&str> = method_samples()
        .iter()
        .filter(|method| method.is_stream())
        .map(|method| method.name())
        .collect();
    assert_eq!(defs["StreamMethodName"]["enum"], json!(streams));
    for entry in reference["methods"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let answer = if entry["stream"] == json!(true) { "item" } else { "result" };
        assert_eq!(defs["MethodParams"]["properties"][name], entry["params"], "{name}");
        assert_eq!(defs["MethodResults"]["properties"][name], entry[answer], "{name}");
    }
    assert_eq!(defs["ProtocolVersion"]["const"], json!(crate::PROTOCOL_VERSION));
}

#[test]
fn the_codegen_file_has_the_remote_members() {
    let doc = codegen_document();
    let defs = &doc["$defs"];
    assert!(defs["HelloResult"]["properties"]["prove"].is_object());
    assert!(defs["ConversationSnapshot"]["properties"]["live"].is_object());
    assert!(defs["LiveTurn"]["properties"]["text_bytes"].is_object());
    assert!(defs["ProveContext"]["properties"]["daemon_tailnet"].is_object());
    let event = serde_json::to_string(&defs["Event"]).unwrap();
    assert!(event.contains("\"local_only\""));
}
