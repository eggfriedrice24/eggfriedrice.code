//! Tool calls of tools that a request does not offer, as plain text.
//!
//! A conversation can hold calls of a tool that its next request does not define: an
//! `edit` call of a Claude model when an OpenAI model goes on with `apply_patch`, an
//! `apply_patch` call the other way round, or a call of a tool that efr no longer has.
//! A provider can refuse a call of a tool that the request does not define, and a model
//! can take it as a tool that it may call. So each request shows such a call, and its
//! result, as text: the history keeps what happened, and the model reads it as a
//! record. Both providers get the same text, because the change happens here, on the
//! canonical messages, before a provider converts them.
//!
//! The text is a pure function of the call and of the request's tool list, so every
//! request with the same tools sends the same bytes for an old call, and the prompt
//! cache reads them. The saved messages keep the call as it was, so a request that
//! offers the tool again, after a switch back, sends the call in its own form. A
//! message with such a call loses its `provider_raw`, which holds the call in the
//! provider's own form.

use std::collections::{HashMap, HashSet};

use efr_provider::{ContentBlock, Message, ToolDefinition};
use serde_json::Value;

/// `messages` for a request that offers `tools`: each call of a tool that `tools` does
/// not name becomes a text block in its place, and each result of such a call becomes
/// a text block in the result's place. A message that changed loses its
/// `provider_raw`. Every other block and message stays as it is.
pub(crate) fn as_offered(mut messages: Vec<Message>, tools: &[ToolDefinition]) -> Vec<Message> {
    let offered: HashSet<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    let mut calls: HashMap<String, String> = HashMap::new();
    for message in &mut messages {
        let mut changed = false;
        for block in &mut message.content {
            let text = match &*block {
                ContentBlock::ToolCall { call_id, name, input, .. }
                    if !offered.contains(name.as_str()) =>
                {
                    calls.insert(call_id.clone(), name.clone());
                    call_text(call_id, name, input)
                }
                ContentBlock::ToolResult { call_id, output, is_error } => {
                    match calls.get(call_id) {
                        Some(name) => result_text(call_id, name, output, *is_error),
                        None => continue,
                    }
                }
                _ => continue,
            };
            *block = ContentBlock::Text { text };
            changed = true;
        }
        if changed {
            message.provider_raw = None;
        }
    }
    messages
}

/// The text in place of a call of the tool `name` with `input`: a freeform call's text
/// as it is, any other input as compact JSON.
fn call_text(call_id: &str, name: &str, input: &Value) -> String {
    let input = match input {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    format!(
        "Earlier tool call `{name}` (id {call_id}), shown as text: this request does not offer \
         the tool. Its input:\n{input}"
    )
}

/// The text in place of the result of a call of the tool `name`.
fn result_text(call_id: &str, name: &str, output: &str, is_error: bool) -> String {
    let failed = if is_error { ", which failed" } else { "" };
    format!("Result of the earlier tool call `{name}` (id {call_id}){failed}:\n{output}")
}

#[cfg(test)]
mod tests;
