use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Request, ToolDefinition};
use crate::{ContentBlock, Message, Role};

fn shell_tool() -> ToolDefinition {
    ToolDefinition {
        name: "shell".to_owned(),
        description: "Run a command in the conversation's shell.".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }),
    }
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
    request.provider_options.insert("reasoning".to_owned(), json!({"effort": "medium"}));

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
        "provider_options": {"reasoning": {"effort": "medium"}},
    });
    assert_eq!(serde_json::to_value(&request).unwrap(), wire);
    assert_eq!(serde_json::from_value::<Request>(wire).unwrap(), request);
}

#[test]
fn decoding_fills_defaults() {
    let request: Request = serde_json::from_value(json!({"model": "m"})).unwrap();
    assert_eq!(request, Request::new("m"));
}
