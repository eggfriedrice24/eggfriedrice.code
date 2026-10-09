use efr_provider::{
    ContentBlock, EditTool, Message, ModelInfo, ProviderError, Request, Role, ToolDefinition,
    ToolGrammar,
};
use insta::assert_snapshot;
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::{Value, json};

use super::{MessagesBody, THINKING_BINDING_BETA, UNANSWERED_CALL, request_body, tool_id};
use crate::{AnthropicConfig, CacheTtl};

mod prefix;

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

fn edit_tool() -> ToolDefinition {
    ToolDefinition::function(
        "edit",
        "Replace a string in a file.",
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean"},
            },
            "required": ["path", "old_string", "new_string"],
        }),
    )
}

/// What the catalog says about Opus 5.5.
fn opus() -> ModelInfo {
    ModelInfo::new("claude-opus-5-5")
        .with_context_window(1_000_000)
        .with_max_context_window(1_000_000)
        .with_max_output_tokens(128_000)
        .with_efforts(["low", "medium", "high", "xhigh", "max"], Some("medium"))
        .with_edit_tool(EditTool::Replace)
}

/// A request to Opus 5.5 with the system prompt and the two tools.
fn request(messages: Vec<Message>) -> Request {
    let mut request = Request::new("claude-opus-5-5");
    request.system = Some("You are efr, an agent in the user's shell.".to_owned());
    request.tools = vec![shell_tool(), edit_tool()];
    request.messages = messages;
    request
}

fn body_with(request: &Request, config: &AnthropicConfig) -> MessagesBody {
    request_body(request, config, Some(&opus())).unwrap()
}

fn body(request: &Request) -> MessagesBody {
    body_with(request, &AnthropicConfig::new())
}

fn pretty(body: &MessagesBody) -> String {
    serde_json::to_string_pretty(body).unwrap()
}

fn value(body: &MessagesBody) -> Value {
    serde_json::from_str(&serde_json::to_string(body).unwrap()).unwrap()
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text { text: text.to_owned() }
}

fn call(call_id: &str, command: &str) -> ContentBlock {
    ContentBlock::ToolCall {
        call_id: call_id.to_owned(),
        name: "shell".to_owned(),
        input: json!({"command": command}),
        freeform: false,
    }
}

fn result(call_id: &str, output: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        call_id: call_id.to_owned(),
        output: output.to_owned(),
        is_error: false,
    }
}

fn user(content: Vec<ContentBlock>) -> Message {
    Message::new(Role::User, content)
}

/// An assistant message that this provider wrote: its canonical content, and the exact
/// content text as `provider_raw`.
fn written(content: Vec<ContentBlock>, raw: &str) -> Message {
    Message::new(Role::Assistant, content).with_provider_raw(Value::String(raw.to_owned()))
}

const THINK_AND_CALL: &str = r#"[{"type":"thinking","thinking":"I list the files.","signature":"c2lnLTE="},{"type":"tool_use","id":"toolu_01","name":"shell","input":{"command":"ls"}}]"#;

#[test]
fn the_first_call_of_a_conversation() {
    let request = request(vec![Message::user("Which files are here?")]);
    assert_snapshot!(pretty(&body(&request)));
}

#[test]
fn a_tool_loop_sends_the_answer_back_as_it_came() {
    let request = request(vec![
        Message::user("Which files are here?"),
        written(vec![call("toolu_01", "ls")], THINK_AND_CALL),
        user(vec![result("toolu_01", "Cargo.toml\nsrc")]),
    ]);
    assert_snapshot!(pretty(&body(&request)));
}

#[test]
fn a_steer_is_a_user_message_of_its_own() {
    let request = request(vec![
        Message::user("Which files are here?"),
        written(vec![call("toolu_01", "ls")], THINK_AND_CALL),
        user(vec![result("toolu_01", "Cargo.toml\nsrc")]),
        Message::user("Only count them."),
    ]);
    assert_snapshot!(pretty(&body(&request)));
}

#[test]
fn the_head_after_a_compaction_keeps_a_message_for_each_part() {
    let request = request(vec![
        Message::user("<fresh_context>The user's shell is in /home/u/p/efr.</fresh_context>"),
        Message::user("<summary>The user asked for the files; there are two.</summary>"),
        Message::user("3 earlier turns are summarised above."),
        Message::assistant("There are two files."),
        Message::user("And the directories?"),
    ]);
    assert_snapshot!(pretty(&body(&request)));
}

#[test]
fn a_summary_request_keeps_five_minutes_on_its_tail() {
    let mut request = request(vec![
        Message::user("Which files are here?"),
        written(vec![call("toolu_01", "ls")], THINK_AND_CALL),
        user(vec![result("toolu_01", "Cargo.toml\nsrc"), text("Summarise the conversation.")]),
    ]);
    request.side_call = true;
    request.max_output_tokens = Some(40_000);
    assert_snapshot!(pretty(&body(&request)));
}

#[test]
fn without_a_system_prompt_the_last_tool_carries_the_marker() {
    let mut request = request(vec![Message::user("Which files are here?")]);
    request.system = None;
    assert_snapshot!(pretty(&body(&request)));
}

#[test]
fn without_tools_there_is_no_tool_choice() {
    let mut request = request(vec![Message::user("Hello.")]);
    request.system = Some(String::new());
    request.tools.clear();
    let body = value(&body(&request));
    assert_eq!(body.get("tools"), None);
    assert_eq!(body.get("tool_choice"), None);
    assert_eq!(body.get("system"), None);
    assert_eq!(
        body["messages"],
        json!([{"role": "user", "content": [{
            "type": "text",
            "text": "Hello.",
            "cache_control": {"type": "ephemeral", "ttl": "1h"},
        }]}])
    );
}

#[test]
fn the_body_puts_the_routing_members_first_and_no_sampling_member() {
    let mut request = request(vec![Message::user("Hello.")]);
    request.provider_options.insert("prompt_cache_key".to_owned(), json!("conversation-1"));
    request.provider_options.insert("reasoning_effort".to_owned(), json!("high"));
    let text = serde_json::to_string(&body(&request)).unwrap();
    assert!(
        text.starts_with(r#"{"model":"claude-opus-5-5","stream":true,"max_tokens":128000,"#),
        "{text}"
    );
    let body: Value = serde_json::from_str(&text).unwrap();
    let members: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        members,
        [
            "max_tokens",
            "messages",
            "model",
            "output_config",
            "stream",
            "system",
            "thinking",
            "tool_choice",
            "tools",
        ]
    );
    assert_eq!(body["output_config"], json!({"effort": "medium"}));
}

#[test]
fn thinking_is_adaptive_and_summarized_and_drops_a_stale_block() {
    let body = value(&body(&request(vec![Message::user("Hello.")])));
    assert_eq!(
        body["thinking"],
        json!({
            "type": "adaptive",
            "display": "summarized",
            "block_binding": {"prefix_mismatch_behavior": "drop_block"},
        })
    );
    assert_eq!(THINKING_BINDING_BETA, "thinking-binding-controls-2026-08-01");
}

#[rstest]
#[case::the_request_wins(Some("high"), &["low", "medium", "high"], Some("low"), Some("high"))]
#[case::else_the_model_default(None, &["low", "medium", "high"], Some("low"), Some("low"))]
#[case::a_model_without_efforts_gets_none(None, &[], None, None)]
#[case::a_model_without_medium_gets_none(None, &["low", "high"], None, None)]
#[case::the_request_still_wins(Some("high"), &[], None, Some("high"))]
fn the_effort_is_sent_only_when_the_request_or_the_model_gives_one(
    #[case] asked: Option<&str>,
    #[case] efforts: &[&str],
    #[case] model_default: Option<&str>,
    #[case] sent: Option<&str>,
) {
    let mut request = request(vec![Message::user("Hello.")]);
    request.effort = asked.map(str::to_owned);
    let model = opus().with_efforts(efforts.iter().copied(), model_default);
    let body = request_body(&request, &AnthropicConfig::new(), Some(&model)).unwrap();
    let expected = sent.map(|effort| json!({ "effort": effort }));
    assert_eq!(value(&body).get("output_config").cloned(), expected);
}

#[test]
fn a_model_that_only_the_config_names_gets_no_effort() {
    let mut request = request(vec![Message::user("Hello.")]);
    request.max_output_tokens = Some(1_000);
    let body = request_body(&request, &AnthropicConfig::new(), None).unwrap();
    assert_eq!(value(&body).get("output_config"), None);
}

#[rstest]
#[case::the_model_limit(None, Some(128_000), Some(128_000))]
#[case::a_smaller_request(Some(40_000), Some(128_000), Some(40_000))]
#[case::a_larger_request_is_cut(Some(200_000), Some(128_000), Some(128_000))]
#[case::a_model_without_a_limit(Some(8_000), None, Some(8_000))]
#[case::no_limit_at_all(None, None, None)]
fn max_tokens_comes_from_the_request_and_the_model(
    #[case] asked: Option<u32>,
    #[case] model_limit: Option<u32>,
    #[case] sent: Option<u64>,
) {
    let mut request = request(vec![Message::user("Hello.")]);
    request.max_output_tokens = asked;
    let mut model = opus();
    model.max_output_tokens = model_limit;
    match (request_body(&request, &AnthropicConfig::new(), Some(&model)), sent) {
        (Ok(body), Some(sent)) => assert_eq!(value(&body)["max_tokens"], json!(sent)),
        (Err(ProviderError::UnknownModel { model }), None) => {
            assert_eq!(model, "claude-opus-5-5");
        }
        (other, _) => panic!("{other:?}"),
    }
}

#[test]
fn a_model_that_the_catalog_does_not_list_needs_a_limit_from_the_request() {
    let mut request = request(vec![Message::user("Hello.")]);
    assert!(matches!(
        request_body(&request, &AnthropicConfig::new(), None),
        Err(ProviderError::UnknownModel { .. })
    ));
    request.max_output_tokens = Some(1_000);
    let body = request_body(&request, &AnthropicConfig::new(), None).unwrap();
    assert_eq!(value(&body)["max_tokens"], json!(1_000));
}

#[test]
fn the_raw_content_goes_back_byte_for_byte() {
    // Members out of the usual order, white space, an empty thinking block, a redacted
    // block and escapes: none of it may change.
    let raw = r#"[ {"signature":"c2ln","thinking":"","type":"thinking"},{"type":"redacted_thinking","data":"ZW5j"},{"type":"text","text":"café \"ok\""}, {"input":{"z":1,"a":[2,1]},"name":"shell","id":"toolu_9","type":"tool_use"} ]"#;
    let request = request(vec![
        Message::user("Go."),
        written(vec![text("café \"ok\""), call("toolu_9", "x")], raw),
        user(vec![result("toolu_9", "done")]),
    ]);
    let text = serde_json::to_string(&body(&request)).unwrap();
    let expected = format!(r#"{{"role":"assistant","content":{raw}}}"#);
    assert!(text.contains(&expected), "{text}");
}

#[test]
fn an_empty_text_block_of_the_raw_content_is_dropped() {
    let raw = r#"[{"type":"thinking","thinking":"a","signature":"s1"},{"type":"text","text":""},{"type":"tool_use","id":"toolu_1","name":"shell","input":{"command":"ls"}}]"#;
    let request = request(vec![
        Message::user("Go."),
        written(vec![call("toolu_1", "ls")], raw),
        user(vec![result("toolu_1", "a.txt")]),
    ]);
    let text = serde_json::to_string(&body(&request)).unwrap();
    let expected = r#"{"role":"assistant","content":[{"type":"thinking","thinking":"a","signature":"s1"},{"type":"tool_use","id":"toolu_1","name":"shell","input":{"command":"ls"}}]}"#;
    assert!(text.contains(expected), "{text}");
}

#[test]
fn raw_content_of_only_an_empty_text_block_sends_no_message() {
    let request = request(vec![
        Message::user("Go."),
        written(vec![text("")], r#"[{"type":"text","text":""}]"#),
        Message::user("Again."),
    ]);
    let body = value(&body(&request));
    let messages = body["messages"].as_array().unwrap();
    let roles: Vec<&Value> = messages.iter().map(|message| &message["role"]).collect();
    assert_eq!(roles, ["user", "user"], "{messages:?}");
}

#[test]
fn merged_raw_messages_keep_each_block_exact() {
    let first = r#"[{"type":"thinking","thinking":"a","signature":"s1"},{"z":1,"type":"text","text":"one"}]"#;
    let second = r#"[{"type":"text","text":"two"}]"#;
    let request = request(vec![
        Message::user("Go."),
        written(vec![text("one")], first),
        written(vec![text("two")], second),
    ]);
    let text = serde_json::to_string(&body(&request)).unwrap();
    let expected = r#"{"role":"assistant","content":[{"type":"thinking","thinking":"a","signature":"s1"},{"z":1,"type":"text","text":"one"},{"type":"text","text":"two"}]}"#;
    assert!(text.contains(expected), "{text}");
}

#[rstest]
#[case::another_providers_items(json!([{"type": "reasoning", "encrypted_content": "x"}]))]
#[case::text_that_is_not_json(json!("not json"))]
#[case::an_empty_array(json!("[]"))]
#[case::a_block_without_a_type(json!(r#"[{"text":"hi"}]"#))]
#[case::a_tool_use_without_an_id(json!(r#"[{"type":"tool_use","name":"shell","input":{}}]"#))]
fn a_foreign_or_damaged_raw_value_falls_back_to_the_content(#[case] raw: Value) {
    let request = request(vec![
        Message::user("Go."),
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Reasoning { text: "Another model's thoughts.".to_owned() },
                text("Done."),
            ],
        )
        .with_provider_raw(raw),
        Message::user("Thanks."),
    ]);
    let body = value(&body(&request));
    assert_eq!(
        body["messages"][1],
        json!({"role": "assistant", "content": [{"type": "text", "text": "Done."}]})
    );
}

#[test]
fn a_rebuilt_answer_keeps_text_and_calls_and_drops_reasoning() {
    let request = request(vec![
        Message::user("Go."),
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Reasoning { text: "Thinking.".to_owned() },
                text(""),
                text("I look."),
                call("call_Qm8s.X2/vR", "ls"),
                ContentBlock::ToolCall {
                    call_id: "call_2".to_owned(),
                    name: "apply_patch".to_owned(),
                    input: json!("*** Begin Patch\n*** End Patch"),
                    freeform: true,
                },
            ],
        ),
        user(vec![
            result("call_Qm8s.X2/vR", "src"),
            ContentBlock::ToolResult {
                call_id: "call_2".to_owned(),
                output: String::new(),
                is_error: true,
            },
        ]),
    ]);
    let body = value(&body(&request));
    assert_eq!(
        body["messages"][1],
        json!({"role": "assistant", "content": [
            {"type": "text", "text": "I look."},
            {"type": "tool_use", "id": "call_Qm8s_X2_vR", "name": "shell", "input": {"command": "ls"}},
            {"type": "tool_use", "id": "call_2", "name": "apply_patch", "input": {"input": "*** Begin Patch\n*** End Patch"}},
        ]})
    );
    assert_eq!(
        body["messages"][2]["content"],
        json!([
            {"type": "tool_result", "tool_use_id": "call_Qm8s_X2_vR", "content": "src"},
            {
                "type": "tool_result",
                "tool_use_id": "call_2",
                "is_error": true,
                "cache_control": {"type": "ephemeral", "ttl": "5m"},
            },
        ])
    );
}

/// The answer of a tool loop that calls `shell` and `apply_patch`, which the request
/// does not offer, after a thinking block.
const THINK_AND_TWO_CALLS: &str = r#"[{"type":"thinking","thinking":"I patch and list.","signature":"c2lnLTI="},{"type":"tool_use","id":"toolu_01","name":"shell","input":{"command":"ls"}},{"type":"tool_use","id":"toolu_02","name":"apply_patch","input":{"input":"*** Begin Patch\n*** End Patch"}}]"#;

#[test]
fn a_raw_call_that_the_request_does_not_offer_goes_as_text_and_the_thinking_stays() {
    let patch = json!({"input": "*** Begin Patch\n*** End Patch"});
    let shown = efr_provider::unoffered_call_text("toolu_02", "apply_patch", &patch);
    let shown_result =
        efr_provider::unoffered_result_text("toolu_02", "apply_patch", "patched", false);
    // The canonical messages as the conversation shows them: the call and its result
    // that the request does not offer are text, and the raw content stays.
    let request = request(vec![
        Message::user("Patch and list."),
        written(vec![call("toolu_01", "ls"), text(&shown)], THINK_AND_TWO_CALLS),
        user(vec![result("toolu_01", "src"), text(&shown_result)]),
    ]);

    let text = serde_json::to_string(&body(&request)).unwrap();

    // The thinking block and the offered call go back exact, each on its own.
    let thinking = r#"{"type":"thinking","thinking":"I patch and list.","signature":"c2lnLTI="}"#;
    let shell = r#"{"type":"tool_use","id":"toolu_01","name":"shell","input":{"command":"ls"}}"#;
    let shown_block = format!(r#"{{"type":"text","text":{}}}"#, json!(shown));
    let expected =
        format!(r#"{{"role":"assistant","content":[{thinking},{shell},{shown_block}]}}"#);
    assert!(text.contains(&expected), "{text}");
    let body = value(&body(&request));
    assert_eq!(body["messages"][1]["content"][0]["type"], json!("thinking"));
    // Only the offered call needs a result; no guard result for the other one.
    let results = body["messages"][2]["content"].as_array().unwrap();
    let ids: Vec<&Value> = results
        .iter()
        .filter(|block| block["type"] == json!("tool_result"))
        .map(|block| &block["tool_use_id"])
        .collect();
    assert_eq!(ids, [&json!("toolu_01")]);
    assert_eq!(results[1]["text"], json!(shown_result));
}

#[test]
fn a_result_shown_as_text_opens_no_turn_and_marks_no_new_anchor() {
    let raw = r#"[{"type":"thinking","thinking":"I patch.","signature":"c2lnLTM="},{"type":"tool_use","id":"toolu_02","name":"apply_patch","input":{"input":"x"}}]"#;
    let shown =
        efr_provider::unoffered_call_text("toolu_02", "apply_patch", &json!({"input": "x"}));
    let shown_result =
        efr_provider::unoffered_result_text("toolu_02", "apply_patch", "patched", false);
    let request = request(vec![
        Message::user("Patch it."),
        written(vec![text(&shown)], raw),
        user(vec![text(&shown_result)]),
    ]);

    let body = value(&body(&request));

    // The prompt keeps the anchor of the first call; the call inside the tool loop puts
    // five minutes on its tail.
    let ttl = |message: usize| {
        let content = body["messages"][message]["content"].as_array().unwrap();
        content.last().unwrap()["cache_control"]["ttl"].clone()
    };
    assert_eq!(ttl(0), json!("1h"), "{body}");
    assert_eq!(ttl(2), json!("5m"), "{body}");
    assert_eq!(body["messages"][1]["content"][0]["type"], json!("thinking"));
}

#[test]
fn tool_results_come_before_text_in_a_user_message() {
    let request = request(vec![
        Message::user("Go."),
        written(vec![call("toolu_01", "ls")], THINK_AND_CALL),
        user(vec![text("Before."), result("toolu_01", "src")]),
        Message::user("After."),
    ]);
    let messages = value(&body(&request))["messages"].clone();
    let content = messages[2]["content"].clone();
    let kinds: Vec<&str> =
        content.as_array().unwrap().iter().map(|block| block["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["tool_result", "text"]);
    assert_eq!(content[1]["text"], json!("Before."));
    // The next user message stays a message of its own.
    assert_eq!(messages[3]["content"][0]["text"], json!("After."));
}

#[test]
fn an_open_tool_call_gets_a_failed_result() {
    let unanswered = json!({
        "type": "tool_result",
        "tool_use_id": "toolu_01",
        "content": UNANSWERED_CALL,
        "is_error": true,
    });
    // The next message holds no result, only text.
    let steered = request(vec![
        Message::user("Go."),
        written(vec![call("toolu_01", "ls")], THINK_AND_CALL),
        user(vec![text("Stop.")]),
    ]);
    let messages = value(&body(&steered))["messages"].clone();
    assert_eq!(messages[2]["content"][0], unanswered);
    assert_eq!(messages[2]["content"][1]["text"], json!("Stop."));
    // No message follows the call.
    let ended =
        request(vec![Message::user("Go."), written(vec![call("toolu_01", "ls")], THINK_AND_CALL)]);
    let messages = value(&body(&ended))["messages"].clone();
    assert_eq!(messages.as_array().unwrap().len(), 3);
    assert_eq!(messages[2]["role"], json!("user"));
    assert_eq!(messages[2]["content"][0]["content"], json!(UNANSWERED_CALL));
}

#[test]
fn empty_text_and_empty_messages_are_left_out() {
    let request = request(vec![
        Message::user("One."),
        Message::assistant(""),
        user(vec![text(""), text("Two.")]),
    ]);
    let messages = value(&body(&request))["messages"].clone();
    // The two user messages stay apart, one run that the API joins: only the last one
    // carries the tail's marker.
    assert_eq!(
        messages,
        json!([
            {"role": "user", "content": [{"type": "text", "text": "One."}]},
            {"role": "user", "content": [
                {"type": "text", "text": "Two.", "cache_control": {"type": "ephemeral", "ttl": "1h"}},
            ]},
        ])
    );
}

#[test]
fn an_image_goes_as_base64() {
    let image: ContentBlock = serde_json::from_value(
        json!({"kind": "image", "media_type": "image/png", "data": "iVBORw0KGgo="}),
    )
    .unwrap();
    let request = request(vec![user(vec![text("What is this?"), image])]);
    let content = value(&body(&request))["messages"][0]["content"].clone();
    assert_eq!(
        content[1],
        json!({
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="},
            "cache_control": {"type": "ephemeral", "ttl": "1h"},
        })
    );
}

#[test]
fn a_freeform_tool_goes_in_its_function_form() {
    let mut request = request(vec![Message::user("Go.")]);
    request.tools = vec![ToolDefinition::freeform(
        "apply_patch",
        "Edit files with a patch.",
        ToolGrammar::lark("start: \"*** Begin Patch\" LF"),
    )];
    let tools = value(&body(&request))["tools"].clone();
    assert_eq!(tools[0]["name"], json!("apply_patch"));
    assert_eq!(tools[0]["input_schema"], efr_provider::freeform_input_schema());
    assert_eq!(tools[0].get("format"), None);
}

#[rstest]
#[case::five_minutes(CacheTtl::FiveMinutes, "5m")]
#[case::one_hour(CacheTtl::OneHour, "1h")]
fn a_fixed_time_to_live_is_on_every_marker(#[case] ttl: CacheTtl, #[case] sent: &str) {
    let request = request(vec![
        Message::user("Go."),
        written(vec![call("toolu_01", "ls")], THINK_AND_CALL),
        user(vec![result("toolu_01", "src")]),
    ]);
    let text =
        serde_json::to_string(&body_with(&request, &AnthropicConfig::new().with_cache_ttl(ttl)))
            .unwrap();
    let marker = format!(r#""cache_control":{{"type":"ephemeral","ttl":"{sent}"}}"#);
    assert_eq!(text.matches(&marker).count(), 3, "{text}");
    assert_eq!(text.matches("cache_control").count(), 3, "{text}");
}

#[rstest]
#[case("toolu_01AbC-_9", "toolu_01AbC-_9")]
#[case("call_Qm8s.X2/vR", "call_Qm8s_X2_vR")]
#[case("fc:é", "fc__")]
#[case("", "_")]
fn a_tool_id_keeps_only_the_characters_that_the_api_takes(#[case] id: &str, #[case] sent: &str) {
    assert_eq!(tool_id(id), sent);
}
