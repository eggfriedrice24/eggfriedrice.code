//! The history: the earlier turns of a conversation as canonical messages, each turn
//! as the model read it, so each request starts with the request before it.
//!
//! A turn reads one snapshot through the store's readers (the conversation's projection
//! row, all of its turns, their saved messages, and a page of the newest events). The
//! list of turns comes from the turns, never from the page. A turn whose exact messages
//! are lost is rebuilt from its events in the page: the prompt, the assistant's text,
//! each tool call with its result, and steering. Those messages carry no provider items
//! and no preamble, because the events do not hold them.
//!
//! The provider's own items (`provider_raw`, such as encrypted reasoning and the ids of
//! its output items) must go back unchanged to the model that made them, so the actor
//! keeps the exact messages of the turns it ran in a [`CachedTurn`], and a cached turn
//! is used instead of the rebuild while the provider and the model are the same
//! ([`ModelKey`]). With another provider or another model the cached messages lose
//! their `provider_raw`, and the model works from the canonical content: the text, the
//! tool calls and their results stay, the other model's encrypted reasoning and item
//! ids do not. opencode does the same (`session/message-v2.ts`, `differentModel`).
//!
//! A turn also saves its exact messages in the store (`efr_store::turn_messages`) with
//! the batch that records its end, and the snapshot reads them back, so after a restart,
//! when the cache is empty, a saved turn takes the cache's place and the request is
//! the same as without the restart. Only a turn with neither is rebuilt.
//!
//! After a compaction with a summary, the history is the fresh context block, the
//! summary, and the messages after the compaction's cut (`efr_store::compactions` reads
//! the newest compaction with a query of its own, never from the page). A newer
//! compaction that only pruned puts the stub in each tool result before its cut.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    Compaction, ConversationId, ConversationSummary, EffectiveSettings, Event, EventEnvelope, Seq,
    TurnId,
};
use efr_provider::{ContentBlock, Message, ProviderId, Role};
use efr_store::Readers;
use efr_store::compactions::{self, LatestCompactions};
use efr_store::conversations::{self, Turn};
use efr_store::events;
use efr_store::turn_messages::{self, TurnMessages};

use crate::ConversationError;
use crate::compaction::{Cut, Placed, Window, gap_note, summary_message};
use crate::context::{BYTES_PER_TOKEN, PRUNED_OUTPUT_STUB};

/// What the model reads for a tool call whose result was never recorded, as when the
/// daemon stopped while the call ran or waited for approval.
pub(crate) const UNFINISHED_CALL: &str = "The call did not finish: the turn ended before \
                                          its result was recorded; it may or may not have \
                                          run.";

/// How much history a request carries. There is no limit on the turns: every turn
/// since the newest summary goes, word for word, so each request starts with the one
/// before it (the README, "The history only grows").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HistoryLimits {
    /// The most events read from the log, not counting the progress of tool output:
    /// the page that the facts of the newest turns come from, and the only source of
    /// a turn that saved no messages. Such a turn whose start is older is left out;
    /// a turn with saved messages never is.
    pub max_events: u32,
    /// The most bytes of history, measured as the JSON of its messages; the oldest
    /// turns are left out first. A turn compacts before it leaves out a turn.
    pub max_bytes: usize,
}

impl HistoryLimits {
    /// Limits with the given values.
    pub fn new(max_events: u32, max_bytes: usize) -> Self {
        HistoryLimits { max_events, max_bytes }
    }

    /// These limits for a model with a window of `window` tokens. Compaction, not the
    /// safety net, must make room, so the net never leaves out turns that the window
    /// can hold: the byte limit is at least twice the window at [`BYTES_PER_TOKEN`]
    /// (the estimate and the model's count differ).
    pub fn for_window(self, window: u64) -> Self {
        let bytes = window.saturating_mul(2 * BYTES_PER_TOKEN);
        let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
        HistoryLimits { max_events: self.max_events, max_bytes: self.max_bytes.max(bytes) }
    }
}

impl Default for HistoryLimits {
    /// 4096 events, 512 KiB.
    fn default() -> Self {
        HistoryLimits::new(4096, 512 * 1024)
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
    /// The prompt with its preamble, then every message of the turn in order: the
    /// messages as the model read them.
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
    /// The events before the page of each finished turn that saved no messages and
    /// started before the page, oldest first, so such a turn is rebuilt whole.
    pub(crate) older: Vec<EventEnvelope>,
    /// The exact messages that earlier turns saved, by turn.
    pub(crate) saved: HashMap<TurnId, CachedTurn>,
    /// The newest compaction with a summary, and the newest of any kind.
    pub(crate) compactions: LatestCompactions,
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
                let turns = conversations::turns(conn, conversation_id)?;
                let page = events::read_turn_history(conn, conversation_id, limits.max_events)?;
                let saved: HashMap<TurnId, CachedTurn> =
                    turn_messages::of_conversation(conn, conversation_id)?
                        .into_iter()
                        .filter_map(decode)
                        .collect();
                let mut older = BTreeMap::new();
                for (from, through) in older_ranges(&turns, &saved, &page) {
                    let events = events::read_turn_range(conn, conversation_id, from, through)?;
                    older.extend(events.into_iter().map(|envelope| (envelope.seq, envelope)));
                }
                let older = older.into_values().collect();
                Ok(Snapshot {
                    summary: conversations::get(conn, conversation_id)?,
                    turns,
                    page,
                    older,
                    saved,
                    compactions: compactions::latest(conn, conversation_id)?,
                })
            })
            .await
            .map_err(ConversationError::from_store)
    }

    /// The user's directory when the newest earlier turn started, from its
    /// `turn_started`; `None` when the page holds no earlier turn. `current` is the
    /// turn being assembled, which never counts.
    pub(crate) fn previous_cwd(&self, current: TurnId) -> Option<&Path> {
        self.page.iter().rev().find_map(|envelope| match &envelope.event {
            Event::TurnStarted { turn_id, cwd, .. } if *turn_id != current => Some(cwd.as_path()),
            _ => None,
        })
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

    /// The newest compaction with a summary, which decides where the history starts.
    pub(crate) fn summary(&self) -> Option<&Compaction> {
        self.compactions.summary.as_ref().map(|stored| &stored.compaction)
    }

    /// The settings of the newest turn that the page holds, from its `turn_started`, for
    /// a manual compaction between turns.
    pub(crate) fn newest_settings(&self) -> Option<&EffectiveSettings> {
        self.page.iter().rev().find_map(|envelope| match &envelope.event {
            Event::TurnStarted { settings, .. } => settings.as_ref(),
            _ => None,
        })
    }

    /// The earlier turns as messages, oldest first, within `limits`; see
    /// [`window`](Self::window).
    #[cfg(test)]
    pub(crate) fn history(
        &self,
        current: TurnId,
        cache: &HashMap<TurnId, Arc<CachedTurn>>,
        key: &ModelKey,
        limits: HistoryLimits,
    ) -> Vec<Message> {
        self.window(Some(current), cache, key, limits, None).messages()
    }

    /// The model's history before the turn `current`, oldest first, within `limits`.
    ///
    /// The list of turns is the conversation's turns, not the page: a turn counts when
    /// it started and finished. `current` is the turn being assembled, which never
    /// counts; a manual compaction between turns has none. A turn's messages are its
    /// exact messages, from the actor's `cache` or the store, else a rebuild from its
    /// events (in the page, or in [`older`](Self::older) when it started before the
    /// page). A turn with exact messages keeps its provider items only when `key`, the
    /// provider and the model of the current turn, answered it.
    ///
    /// After a compaction with a summary, the window starts with `fresh` (the fresh
    /// context block, when given) and the summary, and holds only the messages after
    /// the compaction's cut. A newer compaction that only pruned puts the stub in each
    /// tool result before its own cut.
    ///
    /// The byte limit is a safety net. When it leaves out the oldest turns that no
    /// summary covers, [`Window::omitted`] counts them, so a turn compacts first, and
    /// the head ends with a user message that says how many ([`omitted_note`]), and a
    /// `warn` line gives the count, so neither the model nor the log loses them without
    /// a trace. The summary's own gaps, what it never saw, follow it as a note too
    /// ([`gap_note`]).
    ///
    /// A tool result whose call is not in the window (a cut whose place no longer
    /// matches a turn rebuilt from its events) is left out, because a provider refuses
    /// a result without its call.
    pub(crate) fn window(
        &self,
        current: Option<TurnId>,
        cache: &HashMap<TurnId, Arc<CachedTurn>>,
        key: &ModelKey,
        limits: HistoryLimits,
        fresh: Option<&str>,
    ) -> Window {
        let mut by_turn: HashMap<TurnId, Vec<&EventEnvelope>> = HashMap::new();
        let mut left = left_steers(&self.older);
        left.extend(left_steers(&self.page));
        for envelope in self.older.iter().chain(&self.page) {
            let Some(turn_id) = envelope.event.turn_id() else {
                continue;
            };
            // NOTE: an interrupt sent these steers again as a prompt of their own, which
            // the history holds, or the user took them back; no model call of this turn
            // read them.
            if left.contains(&envelope.seq) {
                continue;
            }
            by_turn.entry(turn_id).or_default().push(envelope);
        }

        let order: HashMap<TurnId, usize> =
            self.turns.iter().enumerate().map(|(at, turn)| (turn.id, at)).collect();
        let position = |turn: TurnId| order.get(&turn).copied();
        let summary = self.summary().and_then(|compaction| {
            let cut = Cut {
                through_turn: compaction.through_turn,
                through_message: compaction.through_message,
            };
            compaction.summary.as_deref().map(|summary| (cut, summary))
        });

        let ran = |turn: &&Turn| {
            Some(turn.id) != current && turn.status.is_finished() && turn.started_at.is_some()
        };
        let mut turns: Vec<Vec<Placed>> = Vec::new();
        for turn in self.turns.iter().filter(ran) {
            let exact = cache.get(&turn.id).map(Arc::as_ref).or_else(|| self.saved.get(&turn.id));
            let messages = match exact {
                Some(exact) if exact.key == *key => exact.messages.clone(),
                Some(exact) => exact.messages.iter().cloned().map(without_raw).collect(),
                None => rebuild(&turn.prompt, by_turn.get(&turn.id).map_or(&[], Vec::as_slice)),
            };
            let placed: Vec<Placed> = messages
                .into_iter()
                .enumerate()
                .map(|(index, message)| Placed {
                    turn: turn.id,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                    message,
                })
                .filter(|placed| summary.is_none_or(|(cut, _)| !cut.covers(placed, position)))
                .collect();
            if !placed.is_empty() {
                turns.push(placed);
            }
        }

        let sizes: Vec<usize> = turns.iter().map(|placed| json_size(placed)).collect();
        let mut total: usize = sizes.iter().sum();
        let mut first = 0;
        while total > limits.max_bytes && first < turns.len() {
            total -= sizes[first];
            first += 1;
        }
        turns.drain(..first);
        let omitted = first;
        let mut placed: Vec<Placed> = turns.into_iter().flatten().collect();
        drop_orphan_results(&mut placed);

        // NOTE: a pruning newer than the summary holds for the results before its cut;
        // an older one lies before the summary's cut.
        let summary_seq = self.compactions.summary.as_ref().map(|stored| stored.seq);
        let pruned = self.compactions.newest.as_ref().filter(|stored| {
            stored.compaction.summary.is_none() && summary_seq.is_none_or(|seq| stored.seq > seq)
        });
        if let Some(pruned) = pruned {
            let cut = Cut {
                through_turn: pruned.compaction.through_turn,
                through_message: pruned.compaction.through_message,
            };
            for item in placed.iter_mut().filter(|item| cut.covers(item, position)) {
                stub_results(&mut item.message);
            }
        }

        let mut head = Vec::new();
        if let Some((_, summary)) = summary {
            head.extend(fresh.map(Message::user));
            head.push(summary_message(summary));
            head.extend(self.summary().and_then(|compaction| {
                gap_note(compaction.omitted_turns, compaction.omitted_messages)
            }));
        }
        if omitted > 0 {
            tracing::warn!(
                omitted,
                max_events = limits.max_events,
                max_bytes = limits.max_bytes,
                "the history leaves out earlier turns"
            );
            head.push(Message::user(omitted_note(omitted)));
        }
        Window { head, placed, omitted: u32::try_from(omitted).unwrap_or(u32::MAX) }
    }
}

/// The ranges of sequence numbers to read for the turns that the page cannot rebuild:
/// each finished turn that saved no messages, from its `prompt_queued` up to the first
/// event of `page` or its own last event. Only a turn from an efr before the saved
/// messages, or one whose saved messages cannot be read back, has such a range.
fn older_ranges(
    turns: &[Turn],
    saved: &HashMap<TurnId, CachedTurn>,
    page: &[EventEnvelope],
) -> Vec<(Seq, Seq)> {
    let first = page.first().map(|envelope| envelope.seq);
    turns
        .iter()
        .filter(|turn| turn.status.is_finished() && turn.started_at.is_some())
        .filter(|turn| !saved.contains_key(&turn.id))
        .filter_map(|turn| {
            let through = match first {
                Some(first) if turn.queued_seq >= first => return None,
                Some(first) => turn.last_seq.min(Seq::new(first.get().saturating_sub(1))),
                None => turn.last_seq,
            };
            Some((turn.queued_seq, through))
        })
        .collect()
}

/// Leaves out each tool result of `placed` whose call is not in `placed`, and a
/// message that holds nothing else.
fn drop_orphan_results(placed: &mut Vec<Placed>) {
    let calls: HashSet<String> = placed
        .iter()
        .flat_map(|placed| &placed.message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let mut dropped = 0_usize;
    placed.retain_mut(|item| {
        let before = item.message.content.len();
        item.message.content.retain(|block| {
            !matches!(block, ContentBlock::ToolResult { call_id, .. } if !calls.contains(call_id))
        });
        let gone = before - item.message.content.len();
        dropped += gone;
        // NOTE: a message that held only such results goes; an empty message that the
        // model sent, such as one with only reasoning, stays.
        gone == 0 || !item.message.content.is_empty()
    });
    if dropped > 0 {
        tracing::warn!(dropped, "the history leaves out tool results whose call it does not hold");
    }
}

/// What the model reads after the summary, or first, when the history leaves out
/// `omitted` earlier turns.
pub(crate) fn omitted_note(omitted: usize) -> String {
    if omitted == 1 {
        "1 earlier turn is omitted.".to_owned()
    } else {
        format!("{omitted} earlier turns are omitted.")
    }
}

/// Gives each tool result of `message` whose output is longer than the stub the stub.
fn stub_results(message: &mut Message) {
    for block in &mut message.content {
        if let ContentBlock::ToolResult { output, .. } = block
            && output.len() > PRUNED_OUTPUT_STUB.len()
        {
            PRUNED_OUTPUT_STUB.clone_into(output);
        }
    }
}

/// The size of the messages of `placed` as JSON, the measure of
/// [`HistoryLimits::max_bytes`].
fn json_size(placed: &[Placed]) -> usize {
    placed
        .iter()
        .map(|placed| serde_json::to_vec(&placed.message).map_or(0, |json| json.len()))
        .sum()
}

/// The `turn_steered` events of `page` that left their turn unread at an interrupt:
/// sent again as a prompt (the `steers` of a `prompt_queued`) or taken back by the user
/// (`steering_withdrawn`).
pub(crate) fn left_steers(page: &[EventEnvelope]) -> HashSet<Seq> {
    page.iter()
        .filter_map(|envelope| match &envelope.event {
            Event::PromptQueued { steers, .. } | Event::SteeringWithdrawn { steers, .. } => {
                Some(steers.iter().copied())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

/// A turn's saved messages as the cache holds them, or `None` when they cannot be read
/// back, such as after a provider was renamed: the turn is then rebuilt from its events,
/// which costs the provider's items and nothing else.
fn decode(saved: TurnMessages) -> Option<(TurnId, CachedTurn)> {
    let turn_id = saved.turn_id;
    let provider = match ProviderId::new(saved.provider) {
        Ok(provider) => provider,
        Err(error) => {
            tracing::warn!(error = %error, %turn_id, "a saved turn names an invalid provider");
            return None;
        }
    };
    let messages = saved
        .messages
        .into_iter()
        .map(serde_json::from_value::<Message>)
        .collect::<Result<Vec<_>, _>>();
    match messages {
        Ok(messages) => {
            Some((turn_id, CachedTurn { key: ModelKey::new(provider, saved.model), messages }))
        }
        // NOTE: the error's text can quote a value of the message, such as a line of a
        // tool's output, so only its category goes to the log.
        Err(error) => {
            tracing::warn!(category = ?error.classify(), %turn_id, "the saved messages of a turn could not be read");
            None
        }
    }
}

/// `message` without the provider's items: the encrypted reasoning and the ids of the
/// provider's output items go, the canonical content stays.
fn without_raw(mut message: Message) -> Message {
    message.provider_raw = None;
    message
}

/// One turn rebuilt from its prompt and its events, in log order.
///
/// Each `assistant_message_completed` starts an assistant message; a tool call joins
/// the assistant message before it unless a tool result came in between; results
/// gather in a user message. Each steer is a user message of its own, where the
/// `steering_delivered` that names it is: the turn records that event just before the
/// model call that sends the steer, so the model read it there and not where the user
/// typed it. A steer that no `steering_delivered` names never reached the model (the
/// turn failed or was interrupted first, or the user took it back), and is left out.
/// Text that was streamed but never completed, as when the daemon stopped mid-answer,
/// is joined from its updates and ends the turn as its last assistant message. A tool
/// call without a recorded result gets an error result, see [`close_open_calls`].
pub(crate) fn rebuild(prompt: &str, events: &[&EventEnvelope]) -> Vec<Message> {
    let mut rebuilt = Rebuilt { messages: vec![Message::user(prompt)], open: Open::None };
    let mut streaming: Option<(u32, String)> = None;
    let mut steers: HashMap<Seq, &str> = HashMap::new();
    // NOTE: a daemon from before `steering_delivered` did not record it. A completed
    // turn of today's daemon reads every steer before it ends, so a completed turn
    // without the event is from such a daemon: it keeps its steers where the user
    // typed them, as its history showed them then.
    let typed_place = events.iter().any(|e| matches!(e.event, Event::TurnCompleted { .. }))
        && !events.iter().any(|e| matches!(e.event, Event::SteeringDelivered { .. }));
    for envelope in events {
        match &envelope.event {
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
            Event::ToolCallStarted { call_id, tool, input, freeform, .. } => {
                let block = ContentBlock::ToolCall {
                    call_id: call_id.to_string(),
                    name: tool.clone(),
                    input: input.clone(),
                    freeform: *freeform,
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
            Event::TurnSteered { text, .. } if typed_place => rebuilt.steer(text),
            Event::TurnSteered { text, .. } => {
                steers.insert(envelope.seq, text);
            }
            Event::SteeringDelivered { steers: delivered, .. } => {
                for text in delivered.iter().filter_map(|seq| steers.get(seq)) {
                    rebuilt.steer(text);
                }
            }
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
