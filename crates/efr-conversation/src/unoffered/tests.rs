use efr_provider::{ContentBlock, EditTool, Message, Role};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::as_offered;
use crate::testing::{FakeToolbox, freeform_message, result_message, tool_message};

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

/// A Claude turn: an `edit` call with its answer's provider items, and its result.
fn claude_turn() -> Vec<Message> {
    let input = json!({"path": "/p/a.md", "old_string": "draft", "new_string": "final"});
    vec![
        Message::user("Fix the notes."),
        tool_message("toolu_01", "edit", &input)
            .with_provider_raw(json!(r#"[{"type":"tool_use","id":"toolu_01"}]"#)),
        result_message("toolu_01", "edited /p/a.md", false),
        Message::assistant("Fixed.").with_provider_raw(json!(r#"[{"type":"text"}]"#)),
    ]
}

#[test]
fn a_call_of_an_offered_tool_stays_as_it_is() {
    let messages = claude_turn();
    let tools = FakeToolbox::tools_for(EditTool::Replace);
    assert_eq!(as_offered(messages.clone(), &tools), messages);
}

#[test]
fn a_call_that_the_request_does_not_offer_and_its_result_become_text() {
    let tools = FakeToolbox::tools_for(EditTool::ApplyPatch);

    let shown = as_offered(claude_turn(), &tools);

    let expected = vec![
        Message::user("Fix the notes."),
        Message::new(
            Role::Assistant,
            vec![text(
                "Earlier tool call `edit` (id toolu_01), shown as text: this request does not \
                 offer the tool. Its input:\n\
                 {\"new_string\":\"final\",\"old_string\":\"draft\",\"path\":\"/p/a.md\"}",
            )],
        ),
        Message::new(
            Role::User,
            vec![text("Result of the earlier tool call `edit` (id toolu_01):\nedited /p/a.md")],
        ),
        // A message without such a call keeps its provider items.
        Message::assistant("Fixed.").with_provider_raw(json!(r#"[{"type":"text"}]"#)),
    ];
    assert_eq!(shown, expected);
}

#[test]
fn a_freeform_call_shows_its_text_and_a_failed_result_says_so() {
    let patch = "*** Begin Patch\n*** Add File: notes.md\n+hello\n*** End Patch";
    let messages = vec![
        Message::user("Add the notes."),
        freeform_message("call_1", "apply_patch", patch),
        result_message("call_1", "the file exists", true),
    ];
    let tools = FakeToolbox::tools_for(EditTool::Replace);

    let shown = as_offered(messages, &tools);

    assert_eq!(
        shown[1].content,
        [text(&format!(
            "Earlier tool call `apply_patch` (id call_1), shown as text: this request does not \
             offer the tool. Its input:\n{patch}"
        ))]
    );
    assert_eq!(
        shown[2].content,
        [text(
            "Result of the earlier tool call `apply_patch` (id call_1), which failed:\nthe file exists"
        )]
    );
}

#[test]
fn only_the_call_that_the_request_does_not_offer_changes_in_a_message_of_several() {
    let messages = vec![
        Message::user("Read and fix."),
        Message::new(
            Role::Assistant,
            vec![
                text("I read it and fix it."),
                call("call_1", "read_file", json!({"path": "a.md"})),
                call("call_2", "removed_tool", json!({})),
            ],
        ),
        Message::new(
            Role::User,
            vec![result("call_1", "contents", false), result("call_2", "", false)],
        ),
    ];
    let tools = FakeToolbox::tools();

    let shown = as_offered(messages, &tools);

    assert_eq!(
        shown[1].content,
        [
            text("I read it and fix it."),
            call("call_1", "read_file", json!({"path": "a.md"})),
            text(
                "Earlier tool call `removed_tool` (id call_2), shown as text: this request does \
                 not offer the tool. Its input:\n{}"
            ),
        ]
    );
    assert_eq!(
        shown[2].content,
        [
            result("call_1", "contents", false),
            text("Result of the earlier tool call `removed_tool` (id call_2):\n"),
        ]
    );
}

#[test]
fn a_result_whose_call_is_not_in_the_messages_stays() {
    let messages = vec![result_message("call_9", "orphan", false)];
    assert_eq!(as_offered(messages.clone(), &[]), messages);
}

#[test]
fn without_tools_every_call_is_text() {
    let shown = as_offered(claude_turn(), &[]);
    let calls = shown
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| {
            matches!(block, ContentBlock::ToolCall { .. } | ContentBlock::ToolResult { .. })
        })
        .count();
    assert_eq!(calls, 0, "{shown:?}");
}

#[test]
fn the_bytes_of_an_old_call_are_the_same_in_every_later_request() {
    let tools = FakeToolbox::tools_for(EditTool::ApplyPatch);
    let mut history = claude_turn();
    let before = serde_json::to_string(&as_offered(history.clone(), &tools)).expect("json");
    history.push(Message::user("And now the tests."));
    history.push(tool_message("call_2", "edit", &json!({"path": "b.md"})));
    history.push(result_message("call_2", "edited b.md", false));
    let after = serde_json::to_string(&as_offered(history.clone(), &tools)).expect("json");

    let open = before.strip_suffix(']').expect("a list");
    assert!(after.starts_with(open), "\n{before}\nis not a prefix of\n{after}");
    let again = serde_json::to_string(&as_offered(history, &tools)).expect("json");
    assert_eq!(after, again);
}
