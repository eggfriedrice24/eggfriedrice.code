use efr_provider::{ContentBlock, Message, Request, Role, ToolDefinition};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::{OutputItem, input_items, provider_raw, request_body, tool_definition};
use crate::{OpenAiConfig, ReasoningMode};

fn shell_tool() -> ToolDefinition {
    ToolDefinition {
        name: "shell".to_owned(),
        description: "Run a command in the user's shell.".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }),
    }
}

fn body(request: &Request, config: &OpenAiConfig) -> Value {
    serde_json::to_value(request_body(request, config)).unwrap()
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text { text: text.to_owned() }
}

fn call(call_id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolCall { call_id: call_id.to_owned(), name: name.to_owned(), input }
}

fn result(call_id: &str, output: &str, is_error: bool) -> ContentBlock {
    ContentBlock::ToolResult { call_id: call_id.to_owned(), output: output.to_owned(), is_error }
}

/// An image block, read from its wire form so the test needs no byte type.
fn image(media_type: &str, base64: &str) -> ContentBlock {
    serde_json::from_value(json!({"kind": "image", "media_type": media_type, "data": base64}))
        .unwrap()
}

#[test]
fn a_subscription_request_has_the_codex_shape() {
    let mut request = Request::new("gpt-5.5");
    request.system = Some("You are efr.".to_owned());
    request.messages = vec![Message::user("Which shell do I use?")];
    request.tools = vec![shell_tool()];
    request.max_output_tokens = Some(4096);
    assert_eq!(
        body(&request, &OpenAiConfig::subscription()),
        json!({
            "model": "gpt-5.5",
            "stream": true,
            "instructions": "You are efr.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Which shell do I use?"}],
            }],
            "tools": [{
                "type": "function",
                "name": "shell",
                "description": "Run a command in the user's shell.",
                "strict": false,
                "parameters": {
                    "type": "object",
                    "properties": {"command": {"type": "string"}},
                    "required": ["command"],
                },
            }],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "reasoning": {"summary": "auto"},
            "store": false,
            "include": ["reasoning.encrypted_content"],
        })
    );
}

#[test]
fn the_body_puts_the_routing_fields_first() {
    let request = Request::new("gpt-5.5");
    let text =
        serde_json::to_string(&request_body(&request, &OpenAiConfig::subscription())).unwrap();
    assert!(text.starts_with(r#"{"model":"gpt-5.5","stream":true,"#), "{text}");
}

#[test]
fn the_api_sends_the_output_limit_and_the_subscription_does_not() {
    let mut request = Request::new("gpt-5.5");
    request.max_output_tokens = Some(4096);
    assert_eq!(body(&request, &OpenAiConfig::api())["max_output_tokens"], json!(4096));
    assert_eq!(body(&request, &OpenAiConfig::subscription()).get("max_output_tokens"), None);
}

#[test]
fn a_model_outside_the_reasoning_families_asks_for_no_reasoning() {
    let body = body(&Request::new("gpt-4.1"), &OpenAiConfig::api());
    assert_eq!(body.get("reasoning"), None);
    assert_eq!(body["include"], json!([]));
}

#[test]
fn the_reasoning_mode_overrides_the_model_family() {
    let always = OpenAiConfig::api().with_reasoning(ReasoningMode::Always);
    let body_always = body(&Request::new("my-fine-tune"), &always);
    assert_eq!(body_always["reasoning"], json!({"summary": "auto"}));
    assert_eq!(body_always["include"], json!(["reasoning.encrypted_content"]));

    let never = OpenAiConfig::subscription().with_reasoning(ReasoningMode::Never);
    let body_never = body(&Request::new("gpt-5.5"), &never);
    assert_eq!(body_never.get("reasoning"), None);
    assert_eq!(body_never["include"], json!([]));
}

#[test]
fn the_config_sets_the_default_effort_and_summary() {
    let config = OpenAiConfig::subscription()
        .with_reasoning_effort(Some("high".to_owned()))
        .with_reasoning_summary(Some("detailed".to_owned()))
        .with_parallel_tool_calls(false);
    let body = body(&Request::new("gpt-5.5"), &config);
    assert_eq!(body["reasoning"], json!({"effort": "high", "summary": "detailed"}));
    assert_eq!(body["parallel_tool_calls"], json!(false));
}

#[test]
fn provider_options_override_the_config() {
    let mut request = Request::new("gpt-5.5");
    request.provider_options = serde_json::from_value(json!({
        "reasoning_effort": "low",
        "reasoning_summary": null,
        "parallel_tool_calls": false,
        "prompt_cache_key": "conversation-0192",
        "service_tier": "flex",
        "text_verbosity": "low",
        "temperature": 0.2,
    }))
    .unwrap();
    let config = OpenAiConfig::subscription().with_reasoning_effort(Some("high".to_owned()));
    let body = body(&request, &config);
    assert_eq!(body["reasoning"], json!({"effort": "low"}));
    assert_eq!(body["parallel_tool_calls"], json!(false));
    assert_eq!(body["prompt_cache_key"], json!("conversation-0192"));
    assert_eq!(body["service_tier"], json!("flex"));
    assert_eq!(body["text"], json!({"verbosity": "low"}));
    assert_eq!(body.get("temperature"), None);
}

#[test]
fn provider_options_of_the_wrong_type_are_ignored() {
    let mut request = Request::new("gpt-5.5");
    request.provider_options = serde_json::from_value(json!({
        "reasoning_effort": 3,
        "reasoning_summary": ["auto"],
        "parallel_tool_calls": "no",
        "prompt_cache_key": 7,
    }))
    .unwrap();
    let config = OpenAiConfig::subscription().with_reasoning_effort(Some("high".to_owned()));
    let body = body(&request, &config);
    assert_eq!(body["reasoning"], json!({"effort": "high", "summary": "auto"}));
    assert_eq!(body["parallel_tool_calls"], json!(true));
    assert_eq!(body.get("prompt_cache_key"), None);
}

#[test]
fn an_empty_system_prompt_sends_no_instructions() {
    let mut request = Request::new("gpt-5.5");
    request.system = Some(String::new());
    assert_eq!(body(&request, &OpenAiConfig::subscription()).get("instructions"), None);
}

#[test]
fn user_text_and_images_form_one_message() {
    let message = Message::new(
        Role::User,
        vec![text("What is on this screen?"), image("image/png", "iVBORw==")],
    );
    assert_eq!(
        input_items(&[message]),
        vec![json!({
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": "What is on this screen?"},
                {"type": "input_image", "image_url": "data:image/png;base64,iVBORw=="},
            ],
        })]
    );
}

#[test]
fn an_assistant_message_without_raw_items_is_rebuilt_in_order() {
    let message = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::Reasoning { text: "The user wants a listing.".to_owned() },
            text("Listing the files."),
            call("call_1", "shell", json!({"command": "ls"})),
            call("call_2", "shell", Value::String("{\"command\": \"ls".to_owned())),
            image("image/png", "iVBORw=="),
        ],
    );
    assert_eq!(
        input_items(&[message]),
        vec![
            json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Listing the files."}],
            }),
            json!({
                "type": "function_call",
                "call_id": "call_1",
                "name": "shell",
                "arguments": "{\"command\":\"ls\"}",
            }),
            json!({
                "type": "function_call",
                "call_id": "call_2",
                "name": "shell",
                "arguments": "{\"command\": \"ls",
            }),
        ]
    );
}

#[test]
fn tool_results_become_function_call_outputs_between_messages() {
    let message = Message::new(
        Role::User,
        vec![
            text("Before."),
            result("call_1", "file.txt", false),
            result("call_2", "permission denied", true),
            text("After."),
            text(""),
        ],
    );
    assert_eq!(
        input_items(&[message]),
        vec![
            json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Before."}]}),
            json!({"type": "function_call_output", "call_id": "call_1", "output": "file.txt"}),
            json!({
                "type": "function_call_output",
                "call_id": "call_2",
                "output": "Error: permission denied",
            }),
            json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "After."}]}),
        ]
    );
}

#[test]
fn raw_items_are_sent_verbatim_in_place_of_the_content() {
    let raw = json!([
        {
            "id": "rs_1",
            "type": "reasoning",
            "encrypted_content": "gAAAAAB-state",
            "summary": [{"type": "summary_text", "text": "Thinking."}],
        },
        {
            "id": "fc_1",
            "type": "function_call",
            "status": "completed",
            "arguments": "{\"command\":\"ls\"}",
            "call_id": "call_1",
            "name": "shell",
        },
    ]);
    let assistant = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::Reasoning { text: "Thinking.".to_owned() },
            call("call_1", "shell", json!({"command": "ls"})),
        ],
    )
    .with_provider_raw(raw.clone());
    let messages = [
        Message::user("List the files."),
        assistant,
        Message::new(Role::User, vec![result("call_1", "file.txt", false)]),
    ];
    let items = input_items(&messages);
    assert_eq!(items.len(), 4);
    assert_eq!(Value::Array(items[1..3].to_vec()), raw);
    assert_eq!(items[3]["type"], json!("function_call_output"));
}

#[rstest]
#[case::not_a_list(json!({"items": []}))]
#[case::empty(json!([]))]
#[case::untyped_item(json!([{"id": "rs_1"}]))]
#[case::not_an_object(json!(["rs_1"]))]
fn unusable_raw_items_fall_back_to_the_content(#[case] raw: Value) {
    let message = Message::assistant("Done.").with_provider_raw(raw);
    assert_eq!(
        input_items(&[message]),
        vec![json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "Done."}],
        })]
    );
}

#[test]
fn an_empty_message_sends_nothing() {
    assert_eq!(
        input_items(&[Message::new(Role::User, Vec::new()), Message::user("")]),
        Vec::<Value>::new()
    );
}

#[test]
fn a_tool_definition_is_a_function_tool() {
    assert_eq!(
        tool_definition(&shell_tool()),
        json!({
            "type": "function",
            "name": "shell",
            "description": "Run a command in the user's shell.",
            "strict": false,
            "parameters": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            },
        })
    );
}

#[test]
fn output_items_read_back_their_canonical_parts() {
    let message = json!({
        "id": "msg_1",
        "type": "message",
        "content": [
            {"type": "output_text", "text": "Part one, "},
            {"type": "refusal", "refusal": "part two."},
            {"type": "input_image", "image_url": "data:"},
        ],
    });
    assert_eq!(
        OutputItem::parse(&message),
        OutputItem::Message { text: "Part one, part two.".to_owned() }
    );

    let reasoning = json!({
        "id": "rs_1",
        "type": "reasoning",
        "summary": [
            {"type": "summary_text", "text": "**One**"},
            {"type": "summary_text", "text": ""},
            {"type": "summary_text", "text": "**Two**"},
        ],
    });
    assert_eq!(
        OutputItem::parse(&reasoning),
        OutputItem::Reasoning { summary: "**One**\n\n**Two**".to_owned() }
    );

    let function_call =
        json!({"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{}"});
    assert_eq!(
        OutputItem::parse(&function_call),
        OutputItem::FunctionCall {
            call_id: "call_1".to_owned(),
            name: "shell".to_owned(),
            arguments: "{}".to_owned(),
        }
    );
}

#[rstest]
#[case::call_without_id(json!({"type": "function_call", "name": "shell"}), "function_call")]
#[case::hosted_tool(json!({"type": "web_search_call", "id": "ws_1"}), "web_search_call")]
#[case::untyped(json!({"id": "x"}), "")]
fn other_output_items_have_no_canonical_form(#[case] item: Value, #[case] kind: &str) {
    assert_eq!(OutputItem::parse(&item), OutputItem::Other { kind: kind.to_owned() });
}

#[test]
fn raw_items_round_trip_through_provider_raw() {
    let items = vec![
        json!({"id": "rs_1", "type": "reasoning", "encrypted_content": "e30=", "summary": []}),
        json!({"id": "msg_1", "type": "message", "role": "assistant", "content": []}),
    ];
    assert_eq!(provider_raw(Vec::new()), None);
    let raw = provider_raw(items.clone()).unwrap();
    let message = Message::assistant("ignored").with_provider_raw(raw);
    assert_eq!(input_items(&[message]), items);
}

#[test]
fn raw_items_on_a_user_message_are_not_used() {
    let raw = json!([{"type": "message", "role": "user", "content": []}]);
    let message = Message::user("Hello.").with_provider_raw(raw);
    assert_eq!(
        input_items(&[message]),
        vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "Hello."}],
        })]
    );
}
