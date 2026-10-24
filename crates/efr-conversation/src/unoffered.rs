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
//! offers the tool again, after a switch back, sends the call in its own form.
//!
//! A message with such a call keeps its `provider_raw`, which holds the call in the
//! provider's own form, and the provider sends that raw call as the same text
//! ([`efr_provider::unoffered_call_text`]). So a Claude message keeps its thinking
//! blocks: the Messages API needs them at the start of the last assistant message of a
//! tool loop.

use std::collections::{HashMap, HashSet};

use efr_provider::{
    ContentBlock, Message, ToolDefinition, unoffered_call_text, unoffered_result_text,
};

use crate::context::json_len;

/// `messages` for a request that offers `tools`: each call of a tool that `tools` does
/// not name becomes a text block in its place, and each result of such a call becomes
/// a text block in the result's place. Every other block, and each message's
/// `provider_raw`, stays as it is.
pub(crate) fn as_offered(mut messages: Vec<Message>, tools: &[ToolDefinition]) -> Vec<Message> {
    let mut offer = Offer::new(tools);
    for message in &mut messages {
        offer.apply(message);
    }
    messages
}

/// The length as JSON of each of `messages` as [`as_offered`] gives them for `tools`,
/// summed, without a copy of the messages: a message that [`as_offered`] does not
/// change is measured as it is, and only a message that it changes is copied first.
pub(crate) fn offered_len<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
    tools: &[ToolDefinition],
) -> u64 {
    let mut offer = Offer::new(tools);
    messages
        .into_iter()
        .map(|message| {
            if !offer.changes(message) {
                return json_len(message);
            }
            let mut shown = message.clone();
            offer.apply(&mut shown);
            json_len(&shown)
        })
        .sum()
}

/// The tools that a request offers, and the calls so far of a tool that it does not
/// offer, with their tool names.
struct Offer<'a> {
    offered: HashSet<&'a str>,
    calls: HashMap<String, String>,
}

impl<'a> Offer<'a> {
    fn new(tools: &'a [ToolDefinition]) -> Self {
        let offered = tools.iter().map(|tool| tool.name.as_str()).collect();
        Offer { offered, calls: HashMap::new() }
    }

    /// True when [`apply`](Self::apply) changes `message`: it holds a call of a tool
    /// that is not offered, or a result of such a call before it. A message without
    /// such a call adds no call, so its results look up the calls before it.
    fn changes(&self, message: &Message) -> bool {
        message.content.iter().any(|block| match block {
            ContentBlock::ToolCall { name, .. } => !self.offered.contains(name.as_str()),
            ContentBlock::ToolResult { call_id, .. } => self.calls.contains_key(call_id),
            _ => false,
        })
    }

    /// Shows each call of a tool that is not offered in `message`, and each result of
    /// such a call, as text.
    fn apply(&mut self, message: &mut Message) {
        for block in &mut message.content {
            let text = match &*block {
                ContentBlock::ToolCall { call_id, name, input, .. }
                    if !self.offered.contains(name.as_str()) =>
                {
                    self.calls.insert(call_id.clone(), name.clone());
                    unoffered_call_text(call_id, name, input)
                }
                ContentBlock::ToolResult { call_id, output, is_error } => {
                    match self.calls.get(call_id) {
                        Some(name) => unoffered_result_text(call_id, name, output, *is_error),
                        None => continue,
                    }
                }
                _ => continue,
            };
            *block = ContentBlock::Text { text };
        }
    }
}

#[cfg(test)]
mod tests;
