//! The drafts of one turn: what the model sent since the last draft, at most once per
//! draft interval (zero in the daemon, so each change goes at once).
//!
//! The drafter sees every provider event before the turn records anything. It keeps
//! the parts themselves in the conversation's [`LiveStatus`]: the text of the current
//! message, with its blocks joined as the completed text joins them, the reasoning of
//! the whole turn, and the input bytes of each tool call of the current answer. A
//! subscriber that lost drafts, or a client that attaches late, reads them there. The
//! drafter keeps its own position in each part: how much of it drafts carried. The
//! first change goes at once; later changes inside the interval wait, and one flush
//! sends every part that changed. While nobody listens on the channel, the drafter
//! sends nothing and its positions stay, so the first draft to a new listener carries
//! everything since the last one sent.
//!
//! The drafter also keeps the newest bytes of the input of each tool call, raw and
//! never parsed, and each tool input draft carries the newest [`INPUT_TAIL`] of them.
//! While the model call waits for its answer, its [`CallDrafts`] send at once what the
//! provider says about it: a connect, the API that took it, and each wait before
//! another attempt.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use efr_protocol::{CompactionTrigger, ContextUse, ConversationId, DraftPart, Seq, TurnId};
use efr_provider::{CallNotice, CallNotices, ProviderEvent};
use jiff::Timestamp;
use tokio::sync::broadcast;

use super::coalesce::Coalescer;
use crate::ConversationDraft;
use crate::drafts::{LiveStatus, LiveToolInput, TurnParts, bold_title};

/// What goes between the reasoning of two model calls of one turn.
const REASONING_BREAK: &str = "\n\n";

/// What goes between two text blocks of one message, as `CompletionBuilder::text`
/// joins them.
const TEXT_BREAK: &str = "\n\n";

/// The most bytes of the newest input of a tool call that one draft carries.
pub(crate) const INPUT_TAIL: usize = 256;

/// The input of one tool call as the model writes it.
#[derive(Debug)]
struct ToolInput {
    /// The provider's id of the call.
    call_id: String,
    tool: String,
    bytes: u64,
    /// The newest bytes of the input as the model wrote them, at most twice
    /// [`INPUT_TAIL`]; they end at `bytes`.
    tail: String,
    /// The byte count that the last draft carried; `None` before the first.
    sent: Option<u64>,
}

impl ToolInput {
    /// The input grew by `more`.
    fn grow(&mut self, more: &str) {
        self.bytes = self.bytes.saturating_add(more.len() as u64);
        self.tail.push_str(more);
        if self.tail.len() > 2 * INPUT_TAIL {
            let cut = ceil_char(&self.tail, self.tail.len() - INPUT_TAIL);
            self.tail.drain(..cut);
        }
    }

    /// The whole input is `input`.
    fn whole(&mut self, input: &str) {
        self.bytes = input.len() as u64;
        let from = ceil_char(input, input.len().saturating_sub(INPUT_TAIL));
        self.tail.clear();
        self.tail.push_str(&input[from..]);
    }

    /// The newest part of the input for a draft: at most [`INPUT_TAIL`] bytes, cut at a
    /// character border, safe to print; `None` before the first byte.
    fn draft_tail(&self) -> Option<String> {
        let from = ceil_char(&self.tail, self.tail.len().saturating_sub(INPUT_TAIL));
        let tail = &self.tail[from..];
        (!tail.is_empty()).then(|| printable(tail))
    }
}

/// The drafts of one turn.
#[derive(Debug)]
pub(super) struct Drafter {
    sender: broadcast::Sender<ConversationDraft>,
    /// The status that a client which attaches late sees at once, and the parts of the
    /// current model call.
    status: Arc<LiveStatus>,
    conversation_id: ConversationId,
    turn_id: TurnId,
    coalescer: Coalescer,
    /// The sequence number of the last event that the turn recorded: the `after_seq`
    /// of its drafts. Atomic, because the turn records through a shared borrow.
    recorded: AtomicU64,
    /// The index of the current message.
    index: u32,
    /// The text grew since the last draft.
    text_changed: bool,
    /// The last block of the current model call is text: the next text delta grows it.
    text_block: bool,
    /// How many bytes of the current message's text drafts carried.
    text_sent: usize,
    /// How many bytes of the turn's reasoning drafts carried.
    reasoning_sent: usize,
    /// True once the current model call sent reasoning.
    reasoning_in_call: bool,
    /// The title of the newest reasoning section.
    title: Option<String>,
    /// Where the next search for a title starts in the turn's reasoning: the start of
    /// the last line, which may still grow.
    title_from: usize,
    /// The tool calls of the current answer, in the order they started.
    calls: Vec<ToolInput>,
}

impl Drafter {
    pub(super) fn new(
        sender: broadcast::Sender<ConversationDraft>,
        status: Arc<LiveStatus>,
        conversation_id: ConversationId,
        turn_id: TurnId,
        interval: Duration,
    ) -> Self {
        Drafter {
            sender,
            status,
            conversation_id,
            turn_id,
            coalescer: Coalescer::new(interval),
            recorded: AtomicU64::new(0),
            index: 0,
            text_changed: false,
            text_block: false,
            text_sent: 0,
            reasoning_sent: 0,
            reasoning_in_call: false,
            title: None,
            title_from: 0,
            calls: Vec::new(),
        }
    }

    /// A new model call starts: its text is message `index`, a new message, or the
    /// current one again from offset 0 when the last call had no text; its tool calls
    /// count from 0.
    pub(super) fn begin_call(&mut self, index: u32) {
        self.index = index;
        self.text_changed = false;
        self.text_block = false;
        self.text_sent = 0;
        self.reasoning_in_call = false;
        self.calls.clear();
        self.with_parts(|parts| {
            parts.index = index;
            parts.text.clear();
            parts.tools.clear();
        });
    }

    /// The model's answer ended: the input of its tool calls is no news any more, the
    /// calls themselves follow as events.
    pub(super) fn answer_ended(&self) {
        if let Some(parts) = self.status.parts().as_mut() {
            parts.tools.clear();
        }
    }

    /// The turn records the whole text of the current message now, and the next one is
    /// `next`. The log has the text from then on, so the live parts drop it first.
    pub(super) fn message_completed(&self, next: u32) {
        if let Some(parts) = self.status.parts().as_mut() {
            parts.index = next;
            parts.text.clear();
        }
    }

    /// The current message ended inside the model call, at a step of the provider's web
    /// tool, and the text after it is message `index`: its text drafts start again at
    /// offset 0. A text draft that waited for the interval is dropped, because the
    /// completion of the message that ended carries its text.
    pub(super) fn next_message(&mut self, index: u32) {
        self.index = index;
        self.text_changed = false;
        self.text_block = false;
        self.text_sent = 0;
        self.with_parts(|parts| {
            parts.index = index;
            parts.text.clear();
        });
    }

    /// The turn recorded events up to `seq`; later drafts come after them.
    pub(super) fn recorded(&self, seq: Seq) {
        self.recorded.fetch_max(seq.get(), Ordering::AcqRel);
        if let Some(parts) = self.status.parts().as_mut() {
            parts.after_seq = parts.after_seq.max(seq);
        }
    }

    /// Takes in one provider event; true when a draft part changed.
    pub(super) fn push(&mut self, event: &ProviderEvent) -> bool {
        match event {
            ProviderEvent::TextDelta { text } if !text.is_empty() => {
                let new_block = !self.text_block;
                self.text_block = true;
                self.text_changed = true;
                self.with_parts(|parts| {
                    if new_block && !parts.text.is_empty() {
                        parts.text.push_str(TEXT_BREAK);
                    }
                    parts.text.push_str(text);
                });
                true
            }
            ProviderEvent::ReasoningDelta { text } if !text.is_empty() => {
                let first = !self.reasoning_in_call;
                self.reasoning_in_call = true;
                self.text_block = false;
                let call_start = self.with_parts(|parts| {
                    if first {
                        if !parts.reasoning.is_empty() {
                            parts.reasoning.push_str(REASONING_BREAK);
                        }
                        parts.call_reasoning = parts.reasoning.len();
                    }
                    parts.reasoning.push_str(text);
                    parts.call_reasoning
                });
                if first {
                    // The title of the last call's reasoning says nothing about this one,
                    // and the search for the next title starts in this call.
                    self.title = None;
                    self.title_from = call_start;
                }
                true
            }
            ProviderEvent::ToolCallStart { call_id, name, .. } => {
                self.text_block = false;
                let call = u32::try_from(self.calls.len()).unwrap_or(u32::MAX);
                self.calls.push(ToolInput {
                    call_id: call_id.clone(),
                    tool: name.clone(),
                    bytes: 0,
                    tail: String::new(),
                    sent: None,
                });
                self.with_parts(|parts| {
                    parts.tools.push(LiveToolInput { call, tool: name.clone(), bytes: 0 });
                });
                true
            }
            ProviderEvent::ToolCallDelta { call_id, arguments } => {
                self.grow_call(call_id, |input| input.grow(arguments))
            }
            ProviderEvent::ToolCallEnd { call_id, arguments } => {
                self.grow_call(call_id, |input| input.whole(arguments))
            }
            // NOTE: a step of the provider's web tool is a border of the text, as in the
            // builder: the text after it is a block of its own.
            ProviderEvent::HostedCallStart { .. } => {
                self.text_block = false;
                false
            }
            _ => false,
        }
    }

    /// A part changed at `now`: sends the drafts at once when the interval allows,
    /// otherwise holds them for [`flush`](Self::flush). Without a listener it does
    /// nothing, so a turn that nobody follows live sets no timer.
    pub(super) fn offer(&mut self, now: Timestamp) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        if self.coalescer.offer(now) {
            self.send();
        }
    }

    /// How long until held drafts must go; `None` when nothing is held.
    pub(super) fn flush_after(&self, now: Timestamp) -> Option<Duration> {
        self.coalescer.flush_after(now)
    }

    /// Sends the held drafts.
    pub(super) fn flush(&mut self, now: Timestamp) {
        self.coalescer.flushed(now);
        self.send();
    }

    /// Sends a draft of every part that changed since the last draft: the reasoning,
    /// then the text of the current message, then the input of each tool call.
    fn send(&mut self) {
        // NOTE: without a receiver nothing moves, so the next listener gets everything
        // since the last draft that went out.
        if self.sender.receiver_count() == 0 {
            return;
        }
        let mut parts = Vec::new();
        {
            let held = self.status.parts();
            let reasoning = held.as_ref().map_or("", |held| held.reasoning.as_str());
            if let Some(delta) =
                reasoning.get(self.reasoning_sent..).filter(|delta| !delta.is_empty())
            {
                find_title(reasoning, &mut self.title_from, &mut self.title);
                parts.push(DraftPart::Reasoning {
                    offset: self.reasoning_sent as u64,
                    delta: delta.to_owned(),
                    title: self.title.clone(),
                });
                self.reasoning_sent = reasoning.len();
            }
            if self.text_changed {
                self.text_changed = false;
                let text = held.as_ref().map_or("", |held| held.text.as_str());
                if let Some(delta) = text.get(self.text_sent..).filter(|delta| !delta.is_empty()) {
                    parts.push(DraftPart::Text {
                        index: self.index,
                        offset: self.text_sent as u64,
                        delta: delta.to_owned(),
                    });
                    self.text_sent = text.len();
                }
            }
        }
        for (position, call) in self.calls.iter_mut().enumerate() {
            if call.sent == Some(call.bytes) {
                continue;
            }
            call.sent = Some(call.bytes);
            parts.push(DraftPart::ToolInput {
                call: u32::try_from(position).unwrap_or(u32::MAX),
                tool: call.tool.clone(),
                bytes: call.bytes,
                tail: call.draft_tail(),
            });
        }
        for part in parts {
            self.emit(part);
        }
    }

    /// Sends how full the context is, at once: before a model call with the estimate,
    /// after it with the provider's count. The status keeps it for a client that
    /// attaches later.
    pub(super) fn context(&self, context: ContextUse) {
        let draft = self.draft(DraftPart::Context(context));
        self.status.hold(&draft);
        self.send_draft(draft);
    }

    /// Sends that the turn compacts its context now. The status keeps it until the
    /// next `context` draft or [`compacted`](Self::compacted).
    pub(super) fn compacting(&self, trigger: CompactionTrigger) {
        let draft = self.draft(DraftPart::Compacting { trigger });
        self.status.hold(&draft);
        self.send_draft(draft);
    }

    /// The compaction ended without a draft after it, as when the turn ends.
    pub(super) fn compacted(&self) {
        self.status.clear(true);
    }

    /// Where the provider tells how the next model call starts. Its drafts go at once,
    /// and the status keeps the newest until the call opened.
    pub(super) fn call_notices(&self) -> CallDrafts {
        CallDrafts {
            sender: self.sender.clone(),
            status: Arc::clone(&self.status),
            conversation_id: self.conversation_id,
            turn_id: self.turn_id,
            after_seq: Seq::new(self.recorded.load(Ordering::Acquire)),
        }
    }

    /// The model call opened its answer, or failed before it: what the provider said
    /// about its start is no news any more.
    pub(super) fn call_opened(&self) {
        self.status.clear_call();
    }

    fn emit(&self, part: DraftPart) {
        // NOTE: without a receiver a send fails and costs nothing; the parts that the
        // drafter holds back stay for the next listener.
        if self.sender.receiver_count() == 0 {
            return;
        }
        self.send_draft(self.draft(part));
    }

    fn draft(&self, part: DraftPart) -> ConversationDraft {
        ConversationDraft {
            conversation_id: self.conversation_id,
            turn_id: self.turn_id,
            after_seq: Seq::new(self.recorded.load(Ordering::Acquire)),
            part,
        }
    }

    fn send_draft(&self, draft: ConversationDraft) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        if let DraftPart::Text { delta, .. } = &draft.part {
            efr_stdx::mark::text("draft.sent", delta.as_bytes());
        }
        // A receiver that went away in the meantime is no error: drafts are best effort.
        let _ = self.sender.send(draft);
    }

    /// Runs `change` on the live parts of this turn, which it starts when the holder
    /// has none of this turn.
    fn with_parts<R>(&self, change: impl FnOnce(&mut TurnParts) -> R) -> R {
        let mut held = self.status.parts();
        let parts = match &mut *held {
            Some(parts) if parts.turn_id == self.turn_id => parts,
            other => {
                let after = Seq::new(self.recorded.load(Ordering::Acquire));
                other.insert(TurnParts::new(self.turn_id, after, self.index))
            }
        };
        change(parts)
    }

    /// Runs `grow` on the input of the newest call `call_id`; false when no such call
    /// started.
    fn grow_call(&mut self, call_id: &str, grow: impl FnOnce(&mut ToolInput)) -> bool {
        let Some(position) = self.calls.iter().rposition(|call| call.call_id == call_id) else {
            return false;
        };
        let Some(call) = self.calls.get_mut(position) else {
            return false;
        };
        grow(call);
        let bytes = call.bytes;
        let call = u32::try_from(position).unwrap_or(u32::MAX);
        self.with_parts(|parts| {
            if let Some(live) = parts.tools.iter_mut().find(|live| live.call == call) {
                live.bytes = bytes;
            }
        });
        true
    }
}

/// The drafts of what the provider says about the start of one model call
/// ([`CallNotice`]). They go at once, as the provider says them, because a client shows
/// them in the place of a wait that would look like a slow model.
#[derive(Debug)]
pub(super) struct CallDrafts {
    sender: broadcast::Sender<ConversationDraft>,
    status: Arc<LiveStatus>,
    conversation_id: ConversationId,
    turn_id: TurnId,
    after_seq: Seq,
}

impl CallNotices for CallDrafts {
    fn notice(&self, notice: CallNotice) {
        let part = match notice {
            CallNotice::Connecting => DraftPart::Connecting,
            CallNotice::Connected => DraftPart::Connected,
            CallNotice::Retrying { attempt, wait, status } => DraftPart::Retrying {
                attempt,
                wait_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                status,
                reason: retry_reason(status),
            },
            _ => return,
        };
        let draft = ConversationDraft {
            conversation_id: self.conversation_id,
            turn_id: self.turn_id,
            after_seq: self.after_seq,
            part,
        };
        if matches!(draft.part, DraftPart::Retrying { .. }) {
            efr_stdx::mark::hit("draft.retrying");
        }
        // NOTE: a client that attaches during the wait sees it from the status.
        self.status.hold(&draft);
        if self.sender.receiver_count() > 0 {
            // A receiver that went away in the meantime is no error: drafts are best
            // effort.
            let _ = self.sender.send(draft);
        }
    }
}

/// Why a model call is sent again, in a few plain words.
fn retry_reason(status: Option<u16>) -> String {
    match status {
        Some(status) => format!("the provider answered {status}"),
        None => "the connection failed".to_owned(),
    }
}

/// `text` with each control character, such as a newline or an escape, as spaces of
/// the same byte length, so it is safe to print and keeps its offsets.
fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(std::iter::repeat_n(' ', c.len_utf8()));
        } else {
            out.push(c);
        }
    }
    out
}

/// The smallest index of `text` at or above `at` that starts a character.
fn ceil_char(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// Reads the lines of `reasoning` that arrived since the last search for a bold title
/// line, from `from` on, and moves `from` to the start of the last line, which may
/// still grow.
fn find_title(reasoning: &str, from: &mut usize, title: &mut Option<String>) {
    let fresh = reasoning.get(*from..).unwrap_or_default();
    if let Some(found) = fresh.rsplit('\n').find_map(bold_title) {
        *title = Some(found.to_owned());
    }
    *from += fresh.rfind('\n').map_or(0, |at| at + 1);
}

#[cfg(test)]
mod tests;
