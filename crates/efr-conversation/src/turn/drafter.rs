//! The drafts of one turn: what the model sent since the last draft, at most once per
//! draft interval.
//!
//! The drafter sees every provider event before the turn records anything, and keeps
//! its own position in each part: the bytes of the current message's text that drafts
//! carried, the reasoning of the whole turn, and the input bytes of each tool call of
//! the current answer. The first change goes at once; later changes inside the
//! interval wait, and one flush sends every part that changed. While nobody listens on
//! the channel, the drafter sends nothing and its positions stay, so the first draft
//! to a new listener carries everything since the last one sent.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use efr_protocol::{ConversationId, DraftPart, Seq, TurnId};
use efr_provider::{CompletionBuilder, ProviderEvent};
use jiff::Timestamp;
use tokio::sync::broadcast;

use super::coalesce::Coalescer;
use crate::ConversationDraft;

/// What goes between the reasoning of two model calls of one turn.
const REASONING_BREAK: &str = "\n\n";

/// The input of one tool call as the model writes it.
#[derive(Debug)]
struct ToolInput {
    /// The provider's id of the call.
    call_id: String,
    tool: String,
    bytes: u64,
    /// The byte count that the last draft carried; `None` before the first.
    sent: Option<u64>,
}

/// The drafts of one turn.
#[derive(Debug)]
pub(super) struct Drafter {
    sender: broadcast::Sender<ConversationDraft>,
    conversation_id: ConversationId,
    turn_id: TurnId,
    coalescer: Coalescer,
    /// The sequence number of the last event that the turn recorded: the `after_seq`
    /// of its drafts. Atomic, because the turn records through a shared borrow.
    recorded: AtomicU64,
    /// The text grew since the last draft.
    text_changed: bool,
    /// How many bytes of the current message's text drafts carried.
    text_sent: usize,
    /// The reasoning of the whole turn so far.
    reasoning: String,
    /// How many bytes of `reasoning` drafts carried.
    reasoning_sent: usize,
    /// True once the current model call sent reasoning.
    reasoning_in_call: bool,
    /// The title of the newest reasoning section.
    title: Option<String>,
    /// Where the next search for a title starts: the start of the last line, which may
    /// still grow.
    title_from: usize,
    /// The tool calls of the current answer, in the order they started.
    calls: Vec<ToolInput>,
}

impl Drafter {
    pub(super) fn new(
        sender: broadcast::Sender<ConversationDraft>,
        conversation_id: ConversationId,
        turn_id: TurnId,
        interval: Duration,
    ) -> Self {
        Drafter {
            sender,
            conversation_id,
            turn_id,
            coalescer: Coalescer::new(interval),
            recorded: AtomicU64::new(0),
            text_changed: false,
            text_sent: 0,
            reasoning: String::new(),
            reasoning_sent: 0,
            reasoning_in_call: false,
            title: None,
            title_from: 0,
            calls: Vec::new(),
        }
    }

    /// A new model call starts: its text is a new message (or continues the current
    /// one at offset 0 when the last call had no text), and its tool calls count from 0.
    pub(super) fn begin_call(&mut self) {
        self.text_changed = false;
        self.text_sent = 0;
        self.reasoning_in_call = false;
        self.calls.clear();
    }

    /// The turn recorded events up to `seq`; later drafts come after them.
    pub(super) fn recorded(&self, seq: Seq) {
        self.recorded.fetch_max(seq.get(), Ordering::AcqRel);
    }

    /// Takes in one provider event; true when a draft part changed.
    pub(super) fn push(&mut self, event: &ProviderEvent) -> bool {
        match event {
            ProviderEvent::TextDelta { text } if !text.is_empty() => {
                self.text_changed = true;
                true
            }
            ProviderEvent::ReasoningDelta { text } if !text.is_empty() => {
                if !self.reasoning_in_call {
                    if !self.reasoning.is_empty() {
                        self.reasoning.push_str(REASONING_BREAK);
                    }
                    // The title of the last call's reasoning says nothing about this one.
                    self.title = None;
                }
                self.reasoning_in_call = true;
                self.reasoning.push_str(text);
                true
            }
            ProviderEvent::ToolCallStart { call_id, name, .. } => {
                self.calls.push(ToolInput {
                    call_id: call_id.clone(),
                    tool: name.clone(),
                    bytes: 0,
                    sent: None,
                });
                true
            }
            ProviderEvent::ToolCallDelta { call_id, arguments } => {
                self.call(call_id).is_some_and(|call| {
                    call.bytes = call.bytes.saturating_add(arguments.len() as u64);
                    true
                })
            }
            ProviderEvent::ToolCallEnd { call_id, arguments } => {
                self.call(call_id).is_some_and(|call| {
                    call.bytes = arguments.len() as u64;
                    true
                })
            }
            _ => false,
        }
    }

    /// A part changed at `now`: sends the drafts at once when the interval allows,
    /// otherwise holds them for [`flush`](Self::flush). Without a listener it does
    /// nothing, so a turn that nobody follows live sets no timer.
    pub(super) fn offer(&mut self, now: Timestamp, builder: &CompletionBuilder, index: u32) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        if self.coalescer.offer(now) {
            self.send(builder, index);
        }
    }

    /// How long until held drafts must go; `None` when nothing is held.
    pub(super) fn flush_after(&self, now: Timestamp) -> Option<Duration> {
        self.coalescer.flush_after(now)
    }

    /// Sends the held drafts.
    pub(super) fn flush(&mut self, now: Timestamp, builder: &CompletionBuilder, index: u32) {
        self.coalescer.flushed(now);
        self.send(builder, index);
    }

    /// Sends a draft of every part that changed since the last draft: the reasoning,
    /// then the text of message `index`, then the input of each tool call.
    fn send(&mut self, builder: &CompletionBuilder, index: u32) {
        // NOTE: without a receiver nothing moves, so the next listener gets everything
        // since the last draft that went out.
        if self.sender.receiver_count() == 0 {
            return;
        }
        if self.reasoning.len() > self.reasoning_sent {
            self.find_title();
            let part = DraftPart::Reasoning {
                offset: self.reasoning_sent as u64,
                delta: self.reasoning[self.reasoning_sent..].to_owned(),
                title: self.title.clone(),
            };
            self.reasoning_sent = self.reasoning.len();
            self.emit(part);
        }
        if self.text_changed {
            self.text_changed = false;
            let text = builder.text();
            if let Some(delta) = text.get(self.text_sent..).filter(|delta| !delta.is_empty()) {
                let part = DraftPart::Text {
                    index,
                    offset: self.text_sent as u64,
                    delta: delta.to_owned(),
                };
                self.text_sent = text.len();
                self.emit(part);
            }
        }
        let mut parts = Vec::new();
        for (position, call) in self.calls.iter_mut().enumerate() {
            if call.sent == Some(call.bytes) {
                continue;
            }
            call.sent = Some(call.bytes);
            parts.push(DraftPart::ToolInput {
                call: u32::try_from(position).unwrap_or(u32::MAX),
                tool: call.tool.clone(),
                bytes: call.bytes,
            });
        }
        for part in parts {
            self.emit(part);
        }
    }

    fn emit(&self, part: DraftPart) {
        let draft = ConversationDraft {
            conversation_id: self.conversation_id,
            turn_id: self.turn_id,
            after_seq: Seq::new(self.recorded.load(Ordering::Acquire)),
            part,
        };
        // A receiver that went away in the meantime is no error: drafts are best effort.
        let _ = self.sender.send(draft);
    }

    fn call(&mut self, call_id: &str) -> Option<&mut ToolInput> {
        self.calls.iter_mut().rev().find(|call| call.call_id == call_id)
    }

    /// Reads the lines that arrived since the last search for a bold title line.
    fn find_title(&mut self) {
        let fresh = &self.reasoning[self.title_from..];
        if let Some(title) = fresh.rsplit('\n').find_map(bold_title) {
            self.title = Some(title.to_owned());
        }
        self.title_from += fresh.rfind('\n').map_or(0, |at| at + 1);
    }
}

/// The text of `line` when the whole line is bold, as in `**Title**`.
fn bold_title(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix("**")?.strip_suffix("**")?.trim();
    (!inner.is_empty() && !inner.contains("**")).then_some(inner)
}

#[cfg(test)]
mod tests;
