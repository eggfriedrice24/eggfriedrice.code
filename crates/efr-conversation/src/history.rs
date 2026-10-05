//! Bounded history: the earlier turns of a conversation as canonical messages.
//!
//! The event log is the source of truth. A turn reads one snapshot through the store's
//! readers (the conversation's projection row, its turns, and the newest events) and
//! rebuilds each earlier turn from its events: the prompt, the assistant's text, each
//! tool call with its result, and steering. Those messages carry no provider items,
//! because the events do not hold them.
//!
//! The provider's own items (`provider_raw`, such as encrypted reasoning and the ids of
//! its output items) must go back unchanged to the model that made them, so the actor
//! keeps the exact messages of the turns it ran in a [`CachedTurn`], and a cached turn
//! is used instead of the rebuild while the provider and the model are the same
//! ([`ModelKey`]). With another provider or another model the cached messages lose
//! their `provider_raw`, and the model works from the canonical content: the text, the
//! tool calls and their results stay, the other model's encrypted reasoning and item
//! ids do not. opencode does the same (`session/message-v2.ts`, `differentModel`).
//! After a restart the cache is empty and every turn is rebuilt.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{ConversationId, ConversationSummary, Event, EventEnvelope, TurnId};
use efr_provider::{ContentBlock, Message, ProviderId, Role};
use efr_store::Readers;
use efr_store::conversations::{self, Turn};
use efr_store::events;

use crate::ConversationError;

/// What the model reads for a tool call whose result was never recorded, as when the
/// daemon stopped while the call ran or waited for approval.
pub(crate) const UNFINISHED_CALL: &str = "The call did not finish: the turn ended before \
                                          its result was recorded; it may or may not have \
                                          run.";

/// How much history a request carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HistoryLimits {
    /// The most earlier turns.
    pub max_turns: usize,
    /// The most events read from the log, not counting the progress of tool output;
    /// a turn whose start is older is left out.
    pub max_events: u32,
    /// The most bytes of history, measured as the JSON of its messages; the oldest
    /// turns are left out first.
    pub max_bytes: usize,
}

impl HistoryLimits {
    /// Limits with the given values.
    pub fn new(max_turns: usize, max_events: u32, max_bytes: usize) -> Self {
        HistoryLimits { max_turns, max_events, max_bytes }
    }
}

impl Default for HistoryLimits {
    /// 50 turns, 4096 events, 512 KiB.
    fn default() -> Self {
        HistoryLimits::new(50, 4096, 512 * 1024)
    }
}

/// The provider and the model that answered a turn. Provider items go back only to the
/// same pair: encrypted reasoning is bound to the model that wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ModelKey {
    pub(crate) provider: ProviderId,
    pub(crate) model: String,
}

impl ModelKey {
    pub(crate) fn new(provider: ProviderId, model: impl Into<String>) -> Self {
        ModelKey { provider, model: model.into() }
    }
}

/// The exact messages of a turn this actor ran, with the provider and the model that
/// answered it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CachedTurn {
    pub(crate) key: ModelKey,
    /// The prompt without the preamble, then every message of the turn in order.
    pub(crate) messages: Vec<Message>,
}

/// What a turn reads from the store before it starts.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    /// The conversation's projection row.
    pub(crate) summary: Option<ConversationSummary>,
    /// Every turn of the conversation, oldest first.
    pub(crate) turns: Vec<Turn>,
    /// The newest events of the conversation, oldest first.
    pub(crate) page: Vec<EventEnvelope>,
}

impl Snapshot {
    /// Reads the snapshot of `conversation_id` in one read transaction.
    pub(crate) async fn read(
        readers: &Readers,
        conversation_id: ConversationId,
        limits: HistoryLimits,
    ) -> Result<Self, ConversationError> {
        readers
            .with(move |conn| {
                Ok(Snapshot {
                    summary: conversations::get(conn, conversation_id)?,
                    turns: conversations::turns(conn, conversation_id)?,
                    page: events::read_turn_history(conn, conversation_id, limits.max_events)?,
                })
            })
            .await
            .map_err(ConversationError::from_store)
    }

    /// Where the conversation's hidden shell is, from the newest shell event in the
    /// page: the directory of a `shell_started` or `cwd_changed`, and nothing after a
    /// `shell_exited`, because the next command starts a new shell in the user's
    /// directory.
    pub(crate) fn agent_cwd(&self) -> Option<PathBuf> {
        self.page
            .iter()
            .rev()
            .find_map(|envelope| match &envelope.event {
                Event::CwdChanged { cwd, .. } | Event::ShellStarted { cwd, .. } => {
                    Some(Some(cwd.clone()))
                }
                Event::ShellExited { .. } => Some(None),
                _ => None,
            })
            .flatten()
    }

    /// The earlier turns as messages, oldest first, within `limits`.
    ///
    /// A turn counts when it has finished and its `turn_started` event is in the page,
    /// so none of its events was cut off. `current` is the turn being assembled, which
    /// never counts. A cached turn keeps its provider items only when `key`, the
    /// provider and the model of the current turn, answered it.
    pub(crate) fn history(
        &self,
        current: TurnId,
        cache: &HashMap<TurnId, Arc<CachedTurn>>,
        key: &ModelKey,
        limits: HistoryLimits,
    ) -> Vec<Message> {
        let mut by_turn: HashMap<TurnId, Vec<&Event>> = HashMap::new();
        let mut started: HashSet<TurnId> = HashSet::new();
        for envelope in &self.page {
            let Some(turn_id) = envelope.event.turn_id() else {
                continue;
            };
            if matches!(envelope.event, Event::TurnStarted { .. }) {
                started.insert(turn_id);
            }
            by_turn.entry(turn_id).or_default().push(&envelope.event);
        }

        let eligible = self.turns.iter().filter(|turn| {
            turn.id != current && turn.status.is_finished() && started.contains(&turn.id)
        });
        let mut turns: Vec<Vec<Message>> = eligible
            .map(|turn| match cache.get(&turn.id) {
                Some(cached) if cached.key == *key => cached.messages.clone(),
                Some(cached) => cached.messages.iter().cloned().map(without_raw).collect(),
                None => rebuild(&turn.prompt, by_turn.get(&turn.id).map_or(&[], Vec::as_slice)),
            })
            .collect();

        let skip = turns.len().saturating_sub(limits.max_turns);
        turns.drain(..skip);
        let sizes: Vec<usize> = turns.iter().map(|messages| json_size(messages)).collect();
        let mut total: usize = sizes.iter().sum();
        let mut first = 0;
        while total > limits.max_bytes && first < turns.len() {
            total -= sizes[first];
            first += 1;
        }
        turns.drain(..first);
        turns.into_iter().flatten().collect()
    }
}

/// `message` without the provider's items: the encrypted reasoning and the ids of the
/// provider's output items go, the canonical content stays.
fn without_raw(mut message: Message) -> Message {
    message.provider_raw = None;
    message
}

/// The size of `messages` as JSON, the measure of [`HistoryLimits::max_bytes`].
fn json_size(messages: &[Message]) -> usize {
    messages.iter().map(|message| serde_json::to_vec(message).map_or(0, |json| json.len())).sum()
}

/// One turn rebuilt from its prompt and its events, in log order.
///
/// Each `assistant_message_completed` starts an assistant message; a tool call joins
/// the assistant message before it unless a tool result came in between; results
/// gather in a user message; steering is a user message of its own. Text that was
/// streamed but never completed, as when the daemon stopped mid-answer, is joined from
/// its updates and ends the turn as its last assistant message. A tool call without a recorded result gets an error
/// result, see [`close_open_calls`].
pub(crate) fn rebuild(prompt: &str, events: &[&Event]) -> Vec<Message> {
    let mut rebuilt = Rebuilt { messages: vec![Message::user(prompt)], open: Open::None };
    let mut streaming: Option<(u32, String)> = None;
    for event in events {
        match event {
            Event::AssistantMessageUpdated { index, offset, delta, .. } => {
                match &mut streaming {
                    Some((open, text)) if open == index && *offset == text.len() as u64 => {
                        text.push_str(delta);
                    }
                    // An update past the text held means the start was never seen.
                    _ if *offset == 0 => streaming = Some((*index, delta.clone())),
                    _ => {}
                }
            }
            Event::AssistantMessageCompleted { index, text, .. } => {
                if streaming.as_ref().is_some_and(|(open, _)| open == index) {
                    streaming = None;
                }
                rebuilt.assistant(ContentBlock::Text { text: text.clone() }, true);
            }
            Event::ToolCallStarted { call_id, tool, input, .. } => {
                let block = ContentBlock::ToolCall {
                    call_id: call_id.to_string(),
                    name: tool.clone(),
                    input: input.clone(),
                };
                rebuilt.assistant(block, false);
            }
            Event::ToolCallCompleted { call_id, output, is_error, .. } => {
                rebuilt.result(ContentBlock::ToolResult {
                    call_id: call_id.to_string(),
                    output: output.clone(),
                    is_error: *is_error,
                });
            }
            Event::TurnSteered { text, .. } => rebuilt.steer(text),
            _ => {}
        }
    }
    if let Some((_, text)) = streaming {
        rebuilt.assistant(ContentBlock::Text { text }, true);
    }
    let mut messages = rebuilt.messages;
    close_open_calls(&mut messages);
    messages
}

/// Gives every tool call in `messages` a result. A provider refuses a request whose
/// tool call has no result (the Responses API answers "No tool output found for
/// function call"), so one turn that stopped mid-call would break every later turn of
/// its conversation. A call without a result gets an error result with
/// [`UNFINISHED_CALL`], in the results message right after its assistant message, or
/// in a new one there.
pub(crate) fn close_open_calls(messages: &mut Vec<Message>) {
    let answered: HashSet<String> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let mut at = 0;
    while at < messages.len() {
        let missing: Vec<ContentBlock> = match messages.get(at) {
            Some(message) if message.role == Role::Assistant => message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolCall { call_id, .. } if !answered.contains(call_id) => {
                        Some(ContentBlock::ToolResult {
                            call_id: call_id.clone(),
                            output: UNFINISHED_CALL.to_owned(),
                            is_error: true,
                        })
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        at += 1;
        if missing.is_empty() {
            continue;
        }
        match messages.get_mut(at) {
            Some(next) if is_results(next) => next.content.extend(missing),
            _ => messages.insert(at, Message::new(Role::User, missing)),
        }
        at += 1;
    }
}

/// True for a user message that holds only tool results.
fn is_results(message: &Message) -> bool {
    message.role == Role::User
        && !message.content.is_empty()
        && message.content.iter().all(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

/// A turn being rebuilt, with the message that the next block may join.
struct Rebuilt {
    messages: Vec<Message>,
    open: Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    None,
    Assistant,
    Results,
}

impl Rebuilt {
    fn assistant(&mut self, block: ContentBlock, new_message: bool) {
        if new_message || self.open != Open::Assistant {
            self.messages.push(Message::new(Role::Assistant, Vec::new()));
            self.open = Open::Assistant;
        }
        self.push(block);
    }

    fn result(&mut self, block: ContentBlock) {
        if self.open != Open::Results {
            self.messages.push(Message::new(Role::User, Vec::new()));
            self.open = Open::Results;
        }
        self.push(block);
    }

    fn steer(&mut self, text: &str) {
        self.messages.push(Message::user(text));
        self.open = Open::None;
    }

    fn push(&mut self, block: ContentBlock) {
        if let Some(message) = self.messages.last_mut() {
            message.content.push(block);
        }
    }
}

#[cfg(test)]
mod tests;
