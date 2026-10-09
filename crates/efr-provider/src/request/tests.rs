use pretty_assertions::assert_eq;
use serde_json::json;

use super::{FREEFORM_INPUT, GrammarSyntax, Request, ToolDefinition, ToolGrammar};
use crate::{ContentBlock, Message, Role};

fn shell_tool() -> ToolDefinition {
    ToolDefinition::function(
        "shell",
        "Run a command in the conversation's shell.",
        json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }),
    )
}

#[test]
fn a_new_request_has_only_a_model() {
    let request = Request::new("gpt-5-codex");
    assert_eq!(request.model, "gpt-5-codex");
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"model": "gpt-5-codex", "messages": []})
    );
}

#[test]
fn a_full_request_round_trips_with_raw_items_intact() {
    let mut request = Request::new("gpt-5-codex");
    request.system = Some("You are efr.".to_owned());
    request.messages = vec![
        Message::user("list the files"),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolCall {
                call_id: "call_1".to_owned(),
                name: "shell".to_owned(),
                input: json!({"command": "ls"}),
                freeform: false,
            }],
        )
        .with_provider_raw(json!([{"type": "function_call", "call_id": "call_1"}])),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                call_id: "call_1".to_owned(),
                output: "Cargo.toml\n".to_owned(),
                is_error: false,
            }],
        ),
    ];
    request.tools = vec![shell_tool()];
    request.max_output_tokens = Some(4096);
    request.effort = Some("medium".to_owned());
    request.side_call = true;
    request.provider_options.insert("prompt_cache_key".to_owned(), json!("conversation-1"));

    let wire = json!({
        "model": "gpt-5-codex",
        "system": "You are efr.",
        "messages": [
            {"role": "user", "content": [{"kind": "text", "text": "list the files"}]},
            {
                "role": "assistant",
                "content": [{"kind": "tool_call", "call_id": "call_1", "name": "shell", "input": {"command": "ls"}}],
                "provider_raw": [{"type": "function_call", "call_id": "call_1"}],
            },
            {"role": "user", "content": [{"kind": "tool_result", "call_id": "call_1", "output": "Cargo.toml\n"}]},
        ],
        "tools": [{
            "name": "shell",
            "description": "Run a command in the conversation's shell.",
            "input_schema": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            },
        }],
        "max_output_tokens": 4096,
        "effort": "medium",
        "side_call": true,
        "provider_options": {"prompt_cache_key": "conversation-1"},
    });
    assert_eq!(serde_json::to_value(&request).unwrap(), wire);
    assert_eq!(serde_json::from_value::<Request>(wire).unwrap(), request);
}

#[test]
fn decoding_fills_defaults() {
    let request: Request = serde_json::from_value(json!({"model": "m"})).unwrap();
    assert_eq!(request, Request::new("m"));
}

#[test]
fn a_function_tool_has_no_grammar_member() {
    let tool = shell_tool();
    assert!(!tool.is_freeform());
    assert!(serde_json::to_value(&tool).unwrap().get("grammar").is_none());
}

#[test]
fn a_freeform_tool_carries_its_grammar_and_the_schema_of_its_function_form() {
    let tool = ToolDefinition::freeform(
        "apply_patch",
        "Edit files with a patch.",
        ToolGrammar::lark("start: \"x\""),
    );
    assert!(tool.is_freeform());
    let wire = serde_json::to_value(&tool).unwrap();
    assert_eq!(wire["grammar"], json!({"syntax": "lark", "definition": "start: \"x\""}));
    assert_eq!(wire["input_schema"]["required"], json!([FREEFORM_INPUT]));
    assert_eq!(wire["input_schema"]["properties"][FREEFORM_INPUT]["type"], json!("string"));
    assert_eq!(serde_json::from_value::<ToolDefinition>(wire).unwrap(), tool);
    assert_eq!(serde_json::to_value(GrammarSyntax::Regex).unwrap(), json!("regex"));
}
