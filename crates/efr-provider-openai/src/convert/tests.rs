use efr_provider::{
    ContentBlock, FREEFORM_INPUT, Message, Request, Role, ToolDefinition, ToolGrammar,
};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::{OutputItem, input_items, provider_raw, request_body, tool_definition};
use crate::catalog::with_extra;
use crate::{Catalog, OpenAiConfig, ReasoningMode};

fn shell_tool() -> ToolDefinition {
    ToolDefinition::function(
        "shell",
        "Run a command in the user's shell.",
        json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }),
    )
}

fn patch_tool() -> ToolDefinition {
    ToolDefinition::freeform(
        "apply_patch",
        "Edit files with a patch.",
        ToolGrammar::lark("start: \"*** Begin Patch\" LF"),
    )
}

fn freeform_call(call_id: &str, text: &str) -> ContentBlock {
    ContentBlock::ToolCall {
        call_id: call_id.to_owned(),
        name: "apply_patch".to_owned(),
        input: json!(text),
        freeform: true,
    }
}

/// The body as the provider builds it, with the built-in catalog and the config's
/// models laid over it.
fn body(request: &Request, config: &OpenAiConfig) -> Value {
    let models = with_extra(Catalog::builtin(config.backend()).models(), config.models());
    let model = models.iter().find(|model| model.id == request.model);
    serde_json::to_value(request_body(request, config, model)).unwrap()
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text { text: text.to_owned() }
}

fn call(call_id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolCall {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        input,
        freeform: false,
    }
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
    let text = serde_json::to_string(&request_body(&request, &OpenAiConfig::subscription(), None))
        .unwrap();
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
fn the_output_limit_of_a_configured_model_wins_over_the_request() {
    let config = OpenAiConfig::api().with_models(vec![
        efr_provider::ModelInfo::new("gpt-next").with_max_output_tokens(64_000),
        efr_provider::ModelInfo::new("gpt-other"),
    ]);
    let mut request = Request::new("gpt-next");
    request.max_output_tokens = Some(4096);
    assert_eq!(body(&request, &config)["max_output_tokens"], json!(64_000));

    let mut other = Request::new("gpt-other");
    other.max_output_tokens = Some(4096);
    assert_eq!(body(&other, &config)["max_output_tokens"], json!(4096));
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
fn the_request_effort_overrides_the_config() {
    let config = OpenAiConfig::subscription().with_reasoning_effort(Some("high".to_owned()));
    let mut request = Request::new("gpt-5.5");
    request.effort = Some("low".to_owned());
    assert_eq!(body(&request, &config)["reasoning"], json!({"effort": "low", "summary": "auto"}));
    // NOTE: the effort is a typed field now; the old option key is an unknown key.
    let mut old = Request::new("gpt-5.5");
    old.provider_options.insert("reasoning_effort".to_owned(), json!("low"));
    assert_eq!(body(&old, &config)["reasoning"], json!({"effort": "high", "summary": "auto"}));
}

#[test]
fn provider_options_override_the_config() {
    let mut request = Request::new("gpt-5.5");
    request.effort = Some("low".to_owned());
    request.provider_options = serde_json::from_value(json!({
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
        input_items(&[message], true, |_| true),
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
        input_items(&[message], true, |_| true),
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
        input_items(&[message], true, |_| true),
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
    let items = input_items(&messages, true, |_| true);
    assert_eq!(items.len(), 4);
    assert_eq!(Value::Array(items[1..3].to_vec()), raw);
    assert_eq!(items[3]["type"], json!("function_call_output"));
}

/// Every object member named `key` anywhere in `value`.
fn members<'a>(value: &'a Value, key: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(object) => {
            for (name, member) in object {
                if name == key {
                    found.push(member);
                }
                members(member, key, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| members(item, key, found)),
        _ => {}
    }
}

#[test]
fn a_body_after_a_model_switch_holds_no_reasoning_or_item_id_of_the_other_model() {
    // NOTE: the conversation sends another model's turns without their provider_raw;
    // this is the body that such messages become.
    let switched = [
        Message::user("List the files."),
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Reasoning { text: "Thinking.".to_owned() },
                call("call_1", "shell", json!({"command": "ls"})),
            ],
        ),
        Message::new(Role::User, vec![result("call_1", "file.txt", false)]),
        Message::assistant("One file."),
        Message::user("And now?"),
    ];
    let mut request = Request::new("gpt-6-sol");
    request.messages = switched.to_vec();
    request.effort = Some("high".to_owned());

    let body = body(&request, &OpenAiConfig::subscription());

    let types: Vec<&str> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        ["message", "function_call", "function_call_output", "message", "message"],
        "the text, the call and its result stay"
    );
    for key in ["id", "encrypted_content"] {
        let mut found = Vec::new();
        members(&body["input"], key, &mut found);
        assert!(found.is_empty(), "{key}: {found:?}");
    }
    assert_eq!(body["reasoning"]["effort"], json!("high"), "the turn's effort");
}

#[rstest]
#[case::not_a_list(json!({"items": []}))]
#[case::empty(json!([]))]
#[case::untyped_item(json!([{"id": "rs_1"}]))]
#[case::not_an_object(json!(["rs_1"]))]
fn unusable_raw_items_fall_back_to_the_content(#[case] raw: Value) {
    let message = Message::assistant("Done.").with_provider_raw(raw);
    assert_eq!(
        input_items(&[message], true, |_| true),
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
        input_items(&[Message::new(Role::User, Vec::new()), Message::user("")], true, |_| true),
        Vec::<Value>::new()
    );
}

#[test]
fn a_tool_definition_is_a_function_tool() {
    assert_eq!(
        tool_definition(&shell_tool(), true),
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
            freeform: false,
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
    assert_eq!(input_items(&[message], true, |_| true), items);
}

#[test]
fn raw_items_with_a_call_of_a_tool_that_the_request_does_not_offer_are_not_sent() {
    let shown = efr_provider::unoffered_call_text("call_2", "edit", &json!({"path": "a.md"}));
    let raw = provider_raw(vec![
        json!({"id": "rs_1", "type": "reasoning", "encrypted_content": "e30=", "summary": []}),
        json!({"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{}"}),
        json!({"type": "function_call", "call_id": "call_2", "name": "edit", "arguments": "{}"}),
    ])
    .unwrap();
    // The canonical content as the conversation shows it: the call of `edit` is text.
    let message =
        Message::new(Role::Assistant, vec![call("call_1", "shell", json!({})), text(&shown)])
            .with_provider_raw(raw.clone());

    let items = input_items(std::slice::from_ref(&message), true, |name| name == "shell");

    assert_eq!(
        items,
        vec![
            json!({"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{}"}),
            json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": shown}],
            }),
        ]
    );
    // Raw items whose calls are all offered still go as they came.
    assert_eq!(input_items(&[message], true, |_| true), raw.as_array().unwrap().clone());
}

#[test]
fn raw_items_on_a_user_message_are_not_used() {
    let raw = json!([{"type": "message", "role": "user", "content": []}]);
    let message = Message::user("Hello.").with_provider_raw(raw);
    assert_eq!(
        input_items(&[message], true, |_| true),
        vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "Hello."}],
        })]
    );
}

#[test]
fn a_freeform_tool_is_a_custom_tool_with_its_grammar() {
    assert_eq!(
        tool_definition(&patch_tool(), true),
        json!({
            "type": "custom",
            "name": "apply_patch",
            "description": "Edit files with a patch.",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: \"*** Begin Patch\" LF",
            },
        })
    );
}

#[test]
fn a_freeform_tool_falls_back_to_its_function_form() {
    let tool = tool_definition(&patch_tool(), false);
    assert_eq!(tool["type"], json!("function"));
    assert_eq!(tool["strict"], json!(false));
    assert_eq!(tool["parameters"]["required"], json!([FREEFORM_INPUT]));
    // A function tool stays a function tool for a model that takes freeform tools.
    assert_eq!(tool_definition(&shell_tool(), true)["type"], json!("function"));
}

#[rstest]
#[case::catalog_model("gpt-5.5", "custom")]
#[case::newest_catalog_model("gpt-6.1-sol", "custom")]
#[case::model_outside_the_catalog("gpt-4.1", "function")]
#[case::unknown_model("some-model", "function")]
fn the_model_decides_the_form_of_a_freeform_tool(#[case] model: &str, #[case] kind: &str) {
    let mut request = Request::new(model);
    request.tools = vec![patch_tool()];
    let config = OpenAiConfig::api();
    assert_eq!(body(&request, &config)["tools"][0]["type"], json!(kind));
}

#[test]
fn a_freeform_call_and_its_result_go_back_as_custom_items() {
    let patch = "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch";
    let messages = [
        Message::new(
            Role::Assistant,
            vec![freeform_call("call_1", patch), call("call_2", "shell", json!({"command": "ls"}))],
        ),
        Message::new(
            Role::User,
            vec![result("call_1", "Success. Deleted: a.txt", false), result("call_2", "x", true)],
        ),
    ];
    assert_eq!(
        input_items(&messages, true, |_| true),
        vec![
            json!({"type": "custom_tool_call", "call_id": "call_1", "name": "apply_patch", "input": patch}),
            json!({"type": "function_call", "call_id": "call_2", "name": "shell", "arguments": "{\"command\":\"ls\"}"}),
            json!({"type": "custom_tool_call_output", "call_id": "call_1", "output": "Success. Deleted: a.txt"}),
            json!({"type": "function_call_output", "call_id": "call_2", "output": "Error: x"}),
        ]
    );
}

#[test]
fn a_freeform_call_goes_back_in_the_function_form_to_a_model_without_freeform_tools() {
    let patch = "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch";
    let messages = [
        Message::new(Role::Assistant, vec![freeform_call("call_1", patch)]),
        Message::new(Role::User, vec![result("call_1", "Success. Deleted: a.txt", false)]),
    ];
    let items = input_items(&messages, false, |_| true);
    assert_eq!(
        items,
        vec![
            json!({"type": "function_call", "call_id": "call_1", "name": "apply_patch", "arguments": json!({"input": patch}).to_string()}),
            json!({"type": "function_call_output", "call_id": "call_1", "output": "Success. Deleted: a.txt"}),
        ]
    );
}

#[test]
fn a_result_of_a_raw_custom_call_is_a_custom_output() {
    let raw = json!([{
        "id": "ctc_1",
        "type": "custom_tool_call",
        "status": "completed",
        "call_id": "call_1",
        "name": "apply_patch",
        "input": "*** Begin Patch\n*** End Patch",
    }]);
    let assistant = Message::new(
        Role::Assistant,
        vec![freeform_call("call_1", "*** Begin Patch\n*** End Patch")],
    )
    .with_provider_raw(raw.clone());
    let messages = [assistant, Message::new(Role::User, vec![result("call_1", "bad", true)])];
    let items = input_items(&messages, true, |_| true);
    assert_eq!(items[0], raw[0]);
    assert_eq!(
        items[1],
        json!({"type": "custom_tool_call_output", "call_id": "call_1", "output": "Error: bad"})
    );
}

#[test]
fn a_custom_call_item_reads_back_as_a_freeform_call() {
    let item = json!({
        "type": "custom_tool_call",
        "call_id": "call_1",
        "name": "apply_patch",
        "input": "*** Begin Patch\n",
    });
    assert_eq!(
        OutputItem::parse(&item),
        OutputItem::FunctionCall {
            call_id: "call_1".to_owned(),
            name: "apply_patch".to_owned(),
            arguments: "*** Begin Patch\n".to_owned(),
            freeform: true,
        }
    );
    let without_name = json!({"type": "custom_tool_call", "call_id": "call_1", "input": ""});
    assert_eq!(
        OutputItem::parse(&without_name),
        OutputItem::Other { kind: "custom_tool_call".to_owned() }
    );
}
