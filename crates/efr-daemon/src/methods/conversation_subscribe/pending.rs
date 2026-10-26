//! The drafts that wait for a free place in one subscriber's lossy room, merged.
//!
//! A subscriber that reads slower than the turn drafts would lose drafts at its full
//! room. Instead, the live side keeps the drafts that do not fit here, at most one per
//! part: the text of one message, the reasoning of the turn, the input of one tool
//! call, the `context` and the `compacting` status. A new draft of a part grows the
//! held one (text and reasoning, at their byte offsets) or replaces it (the others),
//! and the merge goes at the next free place. The subscriber then gets fewer, larger
//! drafts, and every byte of the text and the reasoning.
//!
//! It also knows how far each text and the reasoning went to this subscriber. A draft
//! that repeats bytes it already has is cut to the new ones, so the drafts that it
//! builds again from the turn's live parts after it lost some ([`rebuild`]) and the
//! drafts of the channel that come after them never show a byte twice.
//!
//! [`rebuild`]: PendingDrafts::rebuild

use std::collections::VecDeque;

use efr_conversation::{ConversationDraft, LiveTurn};
use efr_protocol::{DraftPart, Seq, TurnId};

/// The held drafts of one subscription, oldest part first.
#[derive(Debug, Default)]
pub(super) struct PendingDrafts {
    held: VecDeque<ConversationDraft>,
    /// How far the text of the newest message went or waits here: its turn, its index
    /// and its byte end.
    text: Option<(TurnId, u32, u64)>,
    /// How far the reasoning of the newest turn went or waits here: its byte end.
    reasoning: Option<(TurnId, u64)>,
}

impl PendingDrafts {
    /// True when no draft waits.
    pub(super) fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The oldest held draft, to offer next.
    pub(super) fn pop(&mut self) -> Option<ConversationDraft> {
        self.held.pop_front()
    }

    /// Forgets the held drafts that the turn made before `boundary`, an event that
    /// ended what they show.
    pub(super) fn drop_before(&mut self, boundary: Seq) {
        self.held.retain(|draft| draft.after_seq >= boundary);
    }

    /// Holds `draft`: without the bytes that went already, merged with the held draft
    /// of its part.
    pub(super) fn push(&mut self, draft: ConversationDraft) {
        let ConversationDraft { conversation_id, turn_id, after_seq, part } = draft;
        let part = match part {
            DraftPart::Text { index, offset, delta } => {
                let end = match self.text {
                    Some((turn, at, end)) if turn == turn_id && at == index => Some(end),
                    _ => None,
                };
                let Some((offset, delta)) = fresh(end, offset, delta) else {
                    return;
                };
                let new_end = offset.saturating_add(delta.len() as u64);
                self.text = Some((turn_id, index, end.map_or(new_end, |end| end.max(new_end))));
                let grows = self.held.iter_mut().rev().find(|held| {
                    held.turn_id == turn_id
                        && matches!(&held.part, DraftPart::Text { index: at, offset: start, delta: text }
                            if *at == index && start.saturating_add(text.len() as u64) == offset)
                });
                if let Some(held) = grows {
                    if let DraftPart::Text { delta: text, .. } = &mut held.part {
                        text.push_str(&delta);
                    }
                    held.after_seq = held.after_seq.max(after_seq);
                    return;
                }
                DraftPart::Text { index, offset, delta }
            }
            DraftPart::Reasoning { offset, delta, title } => {
                let end = match self.reasoning {
                    Some((turn, end)) if turn == turn_id => Some(end),
                    _ => None,
                };
                let Some((offset, delta)) = fresh(end, offset, delta) else {
                    return;
                };
                let new_end = offset.saturating_add(delta.len() as u64);
                self.reasoning = Some((turn_id, end.map_or(new_end, |end| end.max(new_end))));
                let grows = self.held.iter_mut().rev().find(|held| {
                    held.turn_id == turn_id
                        && matches!(&held.part, DraftPart::Reasoning { offset: start, delta: text, .. }
                            if start.saturating_add(text.len() as u64) == offset)
                });
                if let Some(held) = grows {
                    if let DraftPart::Reasoning { delta: text, title: held_title, .. } =
                        &mut held.part
                    {
                        text.push_str(&delta);
                        // NOTE: the newest draft carries the newest title of its call.
                        *held_title = title;
                    }
                    held.after_seq = held.after_seq.max(after_seq);
                    return;
                }
                DraftPart::Reasoning { offset, delta, title }
            }
            DraftPart::ToolInput { call, tool, bytes } => {
                let same = self.held.iter_mut().find(|held| {
                    held.turn_id == turn_id
                        && matches!(&held.part, DraftPart::ToolInput { call: at, .. } if *at == call)
                });
                if let Some(held) = same {
                    held.part = DraftPart::ToolInput { call, tool, bytes };
                    held.after_seq = held.after_seq.max(after_seq);
                    return;
                }
                DraftPart::ToolInput { call, tool, bytes }
            }
            DraftPart::Context(context) => {
                // NOTE: a `context` draft ends a compaction, so the held `compacting`
                // goes with the older `context`; the new one goes last.
                self.held.retain(|held| {
                    !matches!(held.part, DraftPart::Context(_) | DraftPart::Compacting { .. })
                });
                DraftPart::Context(context)
            }
            DraftPart::Compacting { trigger } => {
                self.held.retain(|held| !matches!(held.part, DraftPart::Compacting { .. }));
                DraftPart::Compacting { trigger }
            }
            other => other,
        };
        self.held.push_back(ConversationDraft { conversation_id, turn_id, after_seq, part });
    }

    /// Holds again what this subscriber lost of the running turn: its status drafts
    /// (`status`, oldest first) and, from its live parts (`live`), the reasoning and
    /// the text past what went here, and the input of each tool call.
    pub(super) fn rebuild(
        &mut self,
        conversation_id: efr_protocol::ConversationId,
        live: Option<LiveTurn>,
        status: Vec<ConversationDraft>,
    ) {
        for draft in status {
            self.push(draft);
        }
        let Some(live) = live else {
            return;
        };
        let LiveTurn { turn_id, after_seq, index, text, reasoning, title, tools } = live;
        let draft = |part| ConversationDraft { conversation_id, turn_id, after_seq, part };
        if !reasoning.is_empty() {
            self.push(draft(DraftPart::Reasoning { offset: 0, delta: reasoning, title }));
        }
        if !text.is_empty() {
            self.push(draft(DraftPart::Text { index, offset: 0, delta: text }));
        }
        for tool in tools {
            self.push(draft(DraftPart::ToolInput {
                call: tool.call,
                tool: tool.tool,
                bytes: tool.bytes,
            }));
        }
    }
}

/// `delta` at `offset` without the bytes before `end`, the end of what went already;
/// `None` when nothing is new. A delta past `end`, or with an unknown `end`, stays
/// whole: the client fills such a gap from the events.
fn fresh(end: Option<u64>, offset: u64, delta: String) -> Option<(u64, String)> {
    let skip = end.map_or(0, |end| end.saturating_sub(offset));
    if skip == 0 {
        return (!delta.is_empty()).then_some((offset, delta));
    }
    let skip = usize::try_from(skip).unwrap_or(usize::MAX);
    if skip >= delta.len() {
        return None;
    }
    match delta.get(skip..) {
        Some(rest) => Some((offset.saturating_add(skip as u64), rest.to_owned())),
        // NOTE: never for drafts of one text, whose ends are character boundaries; the
        // client cuts the overlap by itself.
        None => Some((offset, delta)),
    }
}

#[cfg(test)]
mod tests;
