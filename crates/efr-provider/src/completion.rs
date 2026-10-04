//! A whole answer, collected from a provider's stream.

use std::pin::pin;

use futures::{Stream, StreamExt as _};
use serde_json::{Map, Value};

use crate::{ContentBlock, Message, ProviderError, ProviderEvent, Role, StopReason, TokenUsage};

/// A model's whole answer: what [`Provider::complete`](crate::Provider::complete)
/// returns, and what a [`CompletionBuilder`] makes of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The assistant message, with the provider's items from `Done` as its
    /// `provider_raw`.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// The tokens the call used, when the provider reported them.
    pub usage: Option<TokenUsage>,
}

impl Completion {
    /// Reads `stream` to its end and folds it into a completion. The first error item
    /// ends the read and is returned as it is.
    pub async fn collect<S>(stream: S) -> Result<Completion, ProviderError>
    where
        S: Stream<Item = Result<ProviderEvent, ProviderError>>,
    {
        let mut stream = pin!(stream);
        let mut builder = CompletionBuilder::new();
        while let Some(event) = stream.next().await {
            builder.push(&event?)?;
        }
        builder.finish()
    }
}

/// Folds [`ProviderEvent`]s into a [`Completion`], one event at a time.
///
/// The conversation pushes each event as it forwards it to subscribers, so the stored
/// assistant message and what clients saw are built from the same events. The rules
/// are the stream order documented on [`ProviderEvent`]:
///
/// - consecutive text deltas grow one text block, and so do consecutive reasoning
///   deltas; any other block in between starts a new one;
/// - a tool call takes its place in the message at its `ToolCallStart` and its input
///   from the complete arguments of its `ToolCallEnd`: empty arguments become `{}`, and
///   arguments that are not JSON are kept as a JSON string for the tool layer to
///   report;
/// - the last `Usage` counts; `Raw` events are ignored, even after `Done`;
/// - a call that starts twice, arguments or an end for a call that is not open, a
///   second `Done`, or any other event after `Done` is
///   [`ProviderError::InvalidStream`].
#[derive(Debug, Default)]
pub struct CompletionBuilder {
    slots: Vec<Slot>,
    usage: Option<TokenUsage>,
    done: Option<(StopReason, Option<Value>)>,
}

/// A block of the message being built. A tool call holds its place from its start
/// until its end fills in the input.
#[derive(Debug)]
enum Slot {
    Block(ContentBlock),
    Call { call_id: String, name: String, input: Option<Value> },
}

impl CompletionBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        CompletionBuilder::default()
    }

    /// Adds one event.
    pub fn push(&mut self, event: &ProviderEvent) -> Result<(), ProviderError> {
        if let ProviderEvent::Raw(_) = event {
            return Ok(());
        }
        if self.done.is_some() {
            let problem = match event {
                ProviderEvent::Done { .. } => "a second done",
                _ => "an event after done",
            };
            return Err(ProviderError::InvalidStream { problem });
        }
        match event {
            ProviderEvent::TextDelta { text } => self.append_text(text, false),
            ProviderEvent::ReasoningDelta { text } => self.append_text(text, true),
            ProviderEvent::ToolCallStart { call_id, name } => {
                let seen = self.slots.iter().any(
                    |slot| matches!(slot, Slot::Call { call_id: seen, .. } if seen == call_id),
                );
                if seen {
                    return Err(ProviderError::InvalidStream {
                        problem: "a tool call started twice",
                    });
                }
                self.slots.push(Slot::Call {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    input: None,
                });
            }
            ProviderEvent::ToolCallDelta { call_id, .. } => {
                // NOTE: the deltas only show a call as it is written; the input comes
                // from the complete arguments at the end, so nothing is kept here.
                self.open_call(call_id, "arguments for a tool call that is not open")?;
            }
            ProviderEvent::ToolCallEnd { call_id, arguments } => {
                let input = self.open_call(call_id, "the end of a tool call that is not open")?;
                *input = Some(parse_arguments(arguments));
            }
            ProviderEvent::Usage(usage) => self.usage = Some(*usage),
            ProviderEvent::Done { stop_reason, provider_raw } => {
                self.done = Some((*stop_reason, provider_raw.clone()));
            }
            ProviderEvent::Raw(_) => {}
        }
        Ok(())
    }

    /// The text of the message so far, joined as [`Message::text`] joins it. The
    /// conversation sends it to subscribers while the answer streams.
    pub fn text(&self) -> String {
        let texts: Vec<&str> = self
            .slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Block(ContentBlock::Text { text }) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        texts.join("\n\n")
    }

    /// The completion. Fails with [`ProviderError::Incomplete`] when no `Done` came, and
    /// with [`ProviderError::InvalidStream`] when a tool call never ended.
    pub fn finish(self) -> Result<Completion, ProviderError> {
        let Some((stop_reason, provider_raw)) = self.done else {
            return Err(ProviderError::Incomplete);
        };
        let mut content = Vec::with_capacity(self.slots.len());
        for slot in self.slots {
            content.push(match slot {
                Slot::Block(block) => block,
                Slot::Call { call_id, name, input: Some(input) } => {
                    ContentBlock::ToolCall { call_id, name, input }
                }
                Slot::Call { input: None, .. } => {
                    return Err(ProviderError::InvalidStream {
                        problem: "a tool call never ended",
                    });
                }
            });
        }
        Ok(Completion {
            message: Message { role: Role::Assistant, content, provider_raw },
            stop_reason,
            usage: self.usage,
        })
    }

    fn append_text(&mut self, delta: &str, reasoning: bool) {
        if delta.is_empty() {
            return;
        }
        match self.slots.last_mut() {
            Some(Slot::Block(ContentBlock::Text { text })) if !reasoning => text.push_str(delta),
            Some(Slot::Block(ContentBlock::Reasoning { text })) if reasoning => {
                text.push_str(delta);
            }
            _ => {
                let text = delta.to_owned();
                let block = if reasoning {
                    ContentBlock::Reasoning { text }
                } else {
                    ContentBlock::Text { text }
                };
                self.slots.push(Slot::Block(block));
            }
        }
    }

    /// The input of the started, not yet ended call `call_id`.
    fn open_call(
        &mut self,
        call_id: &str,
        problem: &'static str,
    ) -> Result<&mut Option<Value>, ProviderError> {
        self.slots
            .iter_mut()
            .find_map(|slot| match slot {
                Slot::Call { call_id: id, input: input @ None, .. } if id == call_id => Some(input),
                _ => None,
            })
            .ok_or(ProviderError::InvalidStream { problem })
    }
}

/// The input of a tool call from its complete arguments text.
fn parse_arguments(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(arguments).unwrap_or_else(|_| Value::String(arguments.to_owned()))
}

#[cfg(test)]
mod tests;
