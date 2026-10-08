use efr_provider::{Message, Request};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{Continuation, Plan, plan};
use crate::OpenAiConfig;
use crate::convert::{ResponsesBody, request_body};

fn body(messages: Vec<Message>) -> ResponsesBody {
    let mut request = Request::new("gpt-5.5");
    request.system = Some("You are efr.".to_owned());
    request.messages = messages;
    request_body(&request, &OpenAiConfig::subscription(), None)
}

fn answer_item() -> Value {
    json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hello."}]})
}

/// The continuation after the answer to "hi".
fn after_hi() -> Continuation {
    let first = body(vec![Message::user("hi")]);
    let mut input = first.input().to_vec();
    input.push(answer_item());
    Continuation { settings: first.settings(), input, response_id: "resp_1".to_owned() }
}

/// The history after the answer to "hi", with its raw item, then `next`.
fn history(next: &str) -> Vec<Message> {
    vec![
        Message::user("hi"),
        Message::assistant("Hello.").with_provider_raw(json!([answer_item()])),
        Message::user(next),
    ]
}

#[test]
fn without_a_previous_answer_the_whole_input_goes() {
    assert_eq!(plan(None, &body(history("next"))), Plan::Full);
}

#[test]
fn a_history_that_extends_the_previous_one_sends_only_its_new_items() {
    let last = after_hi();
    let body = body(history("next"));

    let Plan::Incremental { previous, input } = plan(Some(&last), &body) else {
        panic!("expected an incremental plan");
    };

    assert_eq!(previous, "resp_1");
    assert_eq!(input, body.input()[2..].to_vec().as_slice());
    assert_eq!(input.len(), 1);
}

#[test]
fn a_changed_field_sends_the_whole_input() {
    let last = after_hi();
    let mut request = Request::new("gpt-5.5");
    request.system = Some("You are someone else.".to_owned());
    request.messages = history("next");
    let changed = request_body(&request, &OpenAiConfig::subscription(), None);

    assert_eq!(plan(Some(&last), &changed), Plan::Full);
}

#[test]
fn a_rewritten_history_sends_the_whole_input() {
    let last = after_hi();
    let compacted = body(vec![Message::user("Summary: the user said hi."), Message::user("next")]);
    let edited = body(vec![
        Message::user("hi"),
        Message::assistant("Hello, edited."),
        Message::user("next"),
    ]);

    assert_eq!(plan(Some(&last), &compacted), Plan::Full);
    assert_eq!(plan(Some(&last), &edited), Plan::Full);
}

#[test]
fn a_shorter_history_or_an_answer_without_an_id_sends_the_whole_input() {
    let mut last = after_hi();
    assert_eq!(plan(Some(&last), &body(vec![Message::user("hi")])), Plan::Full);

    last.response_id = String::new();
    assert_eq!(plan(Some(&last), &body(history("next"))), Plan::Full);
}

#[test]
fn the_plan_names_itself_for_the_debug_lines() {
    let last = after_hi();
    let body = body(history("next"));
    assert_eq!(plan(None, &body).name(), "full");
    assert_eq!(plan(Some(&last), &body).name(), "incremental");
}
