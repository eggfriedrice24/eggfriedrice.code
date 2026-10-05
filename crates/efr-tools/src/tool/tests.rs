use std::os::unix::fs::symlink;
use std::path::PathBuf;

use efr_scope::Home;
use pretty_assertions::assert_eq;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{
    AccessMode, NoOutput, PathAccess, ToolOutputSink, ToolRequirements, ToolResult, ToolSpec,
    parse_input,
};
use crate::ToolError;
use crate::testing::Fixture;

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

/// A home with `~/.ssh/id_ed25519`, and links to it from the working directory.
fn linked_fixture() -> (Fixture, Home) {
    let fixture = Fixture::new();
    let ssh = fixture.home().join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    std::fs::write(ssh.join("id_ed25519"), "key").unwrap();
    symlink(ssh.join("id_ed25519"), fixture.cwd().join("notes")).unwrap();
    symlink(&ssh, fixture.cwd().join("keys")).unwrap();
    let home = Home::new(fixture.home()).unwrap();
    (fixture, home)
}

fn access(path: impl Into<PathBuf>, mode: AccessMode) -> PathAccess {
    PathAccess { path: path.into(), mode }
}

#[test]
fn a_path_through_a_link_also_declares_what_it_reaches() {
    let (fixture, home) = linked_fixture();
    let key = fixture.home().join(".ssh/id_ed25519");
    let requirements = ToolRequirements::none()
        .with_read(fixture.cwd().join("notes"))
        .with_read_tree(fixture.cwd().join("keys"))
        .with_write(fixture.cwd().join("keys/new"))
        .with_real_paths(&home);
    assert_eq!(
        requirements.paths,
        [
            access(fixture.cwd().join("notes"), AccessMode::Read),
            access(&key, AccessMode::Read),
            access(fixture.cwd().join("keys"), AccessMode::ReadTree),
            access(fixture.home().join(".ssh"), AccessMode::ReadTree),
            access(fixture.cwd().join("keys/new"), AccessMode::Write),
            access(fixture.home().join(".ssh/new"), AccessMode::Write),
        ]
    );
}

#[test]
fn real_paths_and_paths_that_do_not_exist_add_nothing() {
    let (fixture, home) = linked_fixture();
    std::fs::write(fixture.cwd().join("plain"), "x").unwrap();
    let declared = ToolRequirements::none()
        .with_read(fixture.cwd().join("plain"))
        .with_read(fixture.cwd().join("missing/dir/file"))
        .with_read_tree(fixture.home())
        .with_read("relative/path");
    assert_eq!(declared.clone().with_real_paths(&home), declared);
}

#[test]
fn a_home_reached_through_a_link_adds_nothing_and_a_loop_adds_nothing() {
    let fixture = Fixture::new();
    let linked_home = fixture.root().join("linked-home");
    symlink(fixture.home(), &linked_home).unwrap();
    symlink(fixture.cwd().join("b"), fixture.cwd().join("a")).unwrap();
    symlink(fixture.cwd().join("a"), fixture.cwd().join("b")).unwrap();
    let home = Home::new(&linked_home).unwrap();
    let declared = ToolRequirements::none()
        .with_read(linked_home.join(".zshrc"))
        .with_read(fixture.cwd().join("a"));
    assert_eq!(declared.clone().with_real_paths(&home), declared);
}

#[test]
fn a_path_named_twice_is_added_once() {
    let (fixture, home) = linked_fixture();
    let key = fixture.home().join(".ssh/id_ed25519");
    let requirements = ToolRequirements::none()
        .with_read(&key)
        .with_read(fixture.cwd().join("notes"))
        .with_real_paths(&home);
    assert_eq!(
        requirements.paths,
        [access(&key, AccessMode::Read), access(fixture.cwd().join("notes"), AccessMode::Read)]
    );
}

#[test]
fn a_sink_ignores_input_waits_and_lets_hidden_input_wait_unless_it_says_otherwise() {
    let mut seen = Vec::new();
    let mut closure = |tail: &str, bytes: u64| seen.push((tail.to_owned(), bytes));
    closure.input_changed(efr_protocol::InputWait::Hidden, false);
    assert!(closure.can_answer_hidden());
    assert!(!closure.can_answer(), "nobody is known to follow, so the timeout holds");
    let mut none = NoOutput;
    none.input_changed(efr_protocol::InputWait::Visible, true);
    assert!(none.can_answer_hidden());
    assert!(!none.can_answer());
    assert!(seen.is_empty());
}
