use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::ToolRegistry;
use crate::testing::Fixture;
use crate::{
    NoOutput, ReadFileTool, Tool, ToolContext, ToolError, ToolOutputSink, ToolRequirements,
    ToolResult, ToolSpec, WriteFileTool,
};

/// A tool that echoes its input and remembers how often it ran.
#[derive(Debug)]
struct Echo {
    name: String,
    calls: Mutex<u32>,
}

impl Echo {
    fn named(name: impl Into<String>) -> Arc<Self> {
        Arc::new(Echo { name: name.into(), calls: Mutex::new(0) })
    }
}

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(self.name.clone(), "Echoes.", json!({"type": "object"}))
    }

    fn requirements(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<ToolRequirements, ToolError> {
        Ok(ToolRequirements::none().with_network(input["network"] == true))
    }

    fn takes_manual_input(&self, input: &Value) -> bool {
        input["manual"] == true
    }

    async fn invoke(
        &self,
        _ctx: ToolContext,
        input: Value,
        out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        *self.calls.lock().unwrap() += 1;
        out.update("partial", 7);
        Ok(ToolResult::ok(input.to_string()))
    }
}

#[test]
fn specs_come_in_registration_order() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadFileTool::new())).unwrap();
    registry.register(Arc::new(WriteFileTool::new())).unwrap();
    registry.register(Echo::named("echo")).unwrap();
    let names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();
    assert_eq!(names, ["read_file", "write_file", "echo"]);
    assert!(registry.contains("echo"));
    assert!(!registry.contains("shell"));
}

#[test]
fn a_name_is_registered_once() {
    let mut registry = ToolRegistry::new();
    registry.register(Echo::named("echo")).unwrap();
    let error = registry.register(Echo::named("echo")).unwrap_err();
    assert!(matches!(&error, ToolError::DuplicateTool { name } if name == "echo"), "{error:?}");
}

#[test]
fn names_must_be_short_and_plain() {
    let mut registry = ToolRegistry::new();
    for name in [String::new(), "has space".to_owned(), "\u{fc}nicode".to_owned(), "x".repeat(65)] {
        let error = registry.register(Echo::named(name.clone())).unwrap_err();
        assert!(matches!(error, ToolError::InvalidName { .. }), "{name:?}: {error:?}");
    }
    registry.register(Echo::named("mcp.server.tool-1_a")).unwrap();
}

#[test]
fn requirements_are_dispatched_by_name() {
    let fixture = Fixture::new();
    let mut registry = ToolRegistry::new();
    registry.register(Echo::named("echo")).unwrap();
    let requirements =
        registry.requirements("echo", &fixture.context(), &json!({"network": true})).unwrap();
    assert!(requirements.network);
    let error = registry.requirements("nope", &fixture.context(), &json!({})).unwrap_err();
    assert!(matches!(&error, ToolError::UnknownTool { name } if name == "nope"), "{error:?}");
}

#[test]
fn whether_a_call_takes_a_manual_input_is_dispatched_by_name() {
    let mut registry = ToolRegistry::new();
    registry.register(Echo::named("echo")).unwrap();
    registry.register(Arc::new(ReadFileTool::new())).unwrap();
    assert!(registry.takes_manual_input("echo", &json!({"manual": true})));
    assert!(!registry.takes_manual_input("echo", &json!({})));
    assert!(!registry.takes_manual_input("read_file", &json!({"manual": true})), "the default");
    assert!(!registry.takes_manual_input("nope", &json!({"manual": true})));
}

#[tokio::test]
async fn invoke_is_dispatched_by_name_and_streams_output() {
    let fixture = Fixture::new();
    let echo = Echo::named("echo");
    let mut registry = ToolRegistry::new();
    registry.register(Arc::clone(&echo) as Arc<dyn Tool>).unwrap();
    let mut seen = Vec::new();
    let mut out = |tail: &str, bytes: u64| seen.push((tail.to_owned(), bytes));
    let result =
        registry.invoke("echo", fixture.context(), json!({"a": 1}), &mut out).await.unwrap();
    assert_eq!(result.output, r#"{"a":1}"#);
    assert_eq!(seen, [("partial".to_owned(), 7)]);
    assert_eq!(*echo.calls.lock().unwrap(), 1);
    let missing = registry.invoke("nope", fixture.context(), json!({}), &mut NoOutput).await;
    assert!(matches!(missing, Err(ToolError::UnknownTool { .. })), "{missing:?}");
}
