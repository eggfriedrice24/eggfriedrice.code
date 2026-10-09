//! The text in place of a tool call, and of its result, when a request does not offer
//! the call's tool.
//!
//! A conversation can hold calls of a tool that its next request does not define, such
//! as an `edit` call of a Claude model when an OpenAI model goes on with `apply_patch`.
//! The conversation shows each such call and its result as text in the canonical
//! messages, and keeps the message's `provider_raw`. A provider that sends the raw items
//! back sends a raw call of a tool that the request does not offer as
//! [`unoffered_call_text`], or sends the canonical content in place of the raw items.
//! So the text is the same, whoever builds it.

use serde_json::Value;

/// The text in place of the call `call_id` of the tool `name` with `input`, which the
/// request does not offer: a freeform call's text as it is, any other input as compact
/// JSON.
pub fn unoffered_call_text(call_id: &str, name: &str, input: &Value) -> String {
    let input = match input {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    format!(
        "Earlier tool call `{name}` (id {call_id}), shown as text: this request does not offer \
         the tool. Its input:\n{input}"
    )
}

/// The text in place of the result `output` of the call `call_id` of the tool `name`,
/// which the request does not offer.
pub fn unoffered_result_text(call_id: &str, name: &str, output: &str, is_error: bool) -> String {
    let failed = if is_error { ", which failed" } else { "" };
    format!("Result of the earlier tool call `{name}` (id {call_id}){failed}:\n{output}")
}
