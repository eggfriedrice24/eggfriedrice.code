use pretty_assertions::assert_eq;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{AccessMode, PathAccess, ToolRequirements, ToolResult, ToolSpec, parse_input};
use crate::ToolError;

/// An input with one required and one optional field.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[expect(dead_code, reason = "only the schema of the type is under test")]
struct Input {
    /// What to say.
    text: String,
    /// How often.
    #[serde(default)]
    times: Option<u32>,
}

#[test]
fn a_generated_schema_is_an_object_without_schema_and_title_keys() {
    let spec = ToolSpec::for_input::<Input>("say", "Says something.");
    let schema = spec.input_schema.as_object().unwrap();
    assert!(!schema.contains_key("$schema"));
    assert!(!schema.contains_key("title"));
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["text"]));
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(schema["properties"]["text"]["description"], "What to say.");
}

#[test]
fn input_that_does_not_match_names_the_tool() {
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Strict {
        _text: String,
    }
    let error = parse_input::<Strict>("say", &json!({"other": 1})).unwrap_err();
    assert!(matches!(&error, ToolError::InvalidInput { tool, .. } if tool == "say"), "{error:?}");
}

#[test]
fn requirements_collect_paths_in_order() {
    let requirements = ToolRequirements::none()
        .with_read("/etc/hosts")
        .with_write("/tmp/x")
        .with_network(true)
        .with_interactive(true);
    assert_eq!(
        requirements.paths,
        [
            PathAccess { path: "/etc/hosts".into(), mode: AccessMode::Read },
            PathAccess { path: "/tmp/x".into(), mode: AccessMode::Write },
        ]
    );
    assert!(requirements.network && requirements.interactive);
}

#[test]
fn debug_hides_the_command_line() {
    let requirements = ToolRequirements::none().with_command("export TOKEN=hunter2");
    let debug = format!("{requirements:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("<20 bytes>"), "{debug}");
}

#[test]
fn results_say_whether_they_failed() {
    let ok = ToolResult::ok("done").with_exit_code(Some(0));
    assert!(!ok.is_error);
    assert_eq!(ok.exit_code, Some(0));
    let failed = ToolResult::error("no").with_truncated(true);
    assert!(failed.is_error);
    assert!(failed.truncated);
}
