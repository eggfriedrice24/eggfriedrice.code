//! What fills the model's context, for `conversation.context` (`efr context`): the
//! request that the next model call would send, measured part by part, and the counts
//! of the log around it.
//!
//! Every size is an estimate at [`BYTES_PER_TOKEN`](crate::BYTES_PER_TOKEN), as the
//! turn's own estimate is: each block of each message counts for its kind, the frame
//! of a message counts for the kind of its first block, and the rest of the request's
//! JSON (its model, its options, the frame of its lists) counts for `system`. So the
//! parts add up to exactly the estimate of the whole request.
//!
//! Nothing here reads the text of a message into the answer: an item carries a kind,
//! a size, its turn and at most the label that `efr history` shows already.

use std::collections::HashMap;

use efr_protocol::{
    CacheSummary, Compaction, CompactionBrief, CompactionSummary, ContextItem, ContextPart,
    ContextPartKind, CountedUse, Event, EventEnvelope, MAX_LARGEST, TurnId, Usage,
};
use efr_provider::{ContentBlock, Message, Request, Role};
use serde_json::Value;

use crate::compaction::{Window, with_window};
use crate::context::{PRUNED_OUTPUT_STUB, estimate_tokens};

/// How the live-state preamble of a prompt starts (`preamble.rs`, `LiveState::render`).
const PREAMBLE_OPEN: &str = "<live_state>";

/// How the text of a job notice starts (`notice.rs`, `notice_text`).
const NOTICE_OPEN: &str = "<job id=\"";

/// The keys of a tool's input whose value names what a call does, in the order that
/// `efr history` looks for them.
const LABEL_KEYS: &[&str] = &["command", "cmd", "path", "file", "url", "query"];

/// The most characters of a label.
const LABEL_CHARS: usize = 120;

/// The tool whose input is a patch; its label is the files of the patch.
const PATCH_TOOL: &str = "apply_patch";

/// The marker lines of a patch that name a file.
const PATCH_FILE_MARKERS: &[&str] = &["*** Add File:", "*** Update File:", "*** Delete File:"];

/// The next request, measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Measured {
    /// The estimated tokens of the whole request: the sum of `parts`.
    pub(crate) estimate: u64,
    /// One part per kind that has something in it, in the order of
    /// [`ContextPartKind::ALL`].
    pub(crate) parts: Vec<ContextPart>,
    /// The largest items, largest first.
    pub(crate) largest: Vec<ContextItem>,
}

/// Measures the request that `base` sends with `window`. `turns` gives the number of
/// each turn in the conversation, counted from 1.
pub(crate) fn measure(base: &Request, window: &Window, turns: &HashMap<TurnId, u32>) -> Measured {
    let request = with_window(base, window);
    let total = json_len(&request);
    let mut tally = Tally::default();

    if let Some(system) = &request.system {
        tally.add(ContextPartKind::System, json_len(system), 1);
    }
    tally.add(
        ContextPartKind::Tools,
        if request.tools.is_empty() { 0 } else { json_len(&request.tools) },
        u32::try_from(request.tools.len()).unwrap_or(u32::MAX),
    );

    // NOTE: the request's messages are the window's, one for one and block for block,
    // with a call of a tool that the request does not offer as text; the kind comes
    // from the window, the size from what the request sends.
    let originals = window.messages();
    // NOTE: `None` for a message of the head, before the turns.
    let places: Vec<Option<Option<u32>>> = window
        .head
        .iter()
        .map(|_| None)
        .chain(window.placed.iter().map(|placed| Some(turns.get(&placed.turn).copied())))
        .collect();
    let calls = call_labels(&originals);
    for ((original, sent), place) in originals.iter().zip(&request.messages).zip(places) {
        match place {
            None => {
                let bytes = json_len(sent);
                tally.add(ContextPartKind::Summary, bytes, 1);
                tally.item(Item::new(ContextPartKind::Summary, bytes, None));
            }
            Some(turn) => tally.message(original, sent, turn, &calls),
        }
    }

    let counted: u64 = tally.bytes.values().sum();
    // NOTE: the request's own fields and the frame of its lists belong to no message.
    tally.add(ContextPartKind::System, total.saturating_sub(counted), 0);
    tally.finish()
}

/// The label and the tool of each call in `messages`, by call id.
fn call_labels(messages: &[Message]) -> HashMap<String, (String, Option<String>)> {
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolCall { call_id, name, input, .. } => {
                Some((call_id.clone(), (name.clone(), call_label(name, input))))
            }
            _ => None,
        })
        .collect()
}

/// What `efr history` shows for a call of `tool` with `input`: the files of a patch,
/// else the first of [`LABEL_KEYS`] that the input has, as one line of at most
/// [`LABEL_CHARS`] characters. `None` when the input names nothing.
pub(crate) fn call_label(tool: &str, input: &Value) -> Option<String> {
    if tool == PATCH_TOOL {
        let text = match input {
            Value::String(text) => Some(text.as_str()),
            Value::Object(members) => members.get("input").and_then(Value::as_str),
            _ => None,
        };
        if let Some(text) = text {
            let files: Vec<&str> = text
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    PATCH_FILE_MARKERS.iter().find_map(|marker| line.strip_prefix(marker))
                })
                .map(str::trim)
                .collect();
            return (!files.is_empty()).then(|| one_line(&files.join(", ")));
        }
        return None;
    }
    let Value::Object(members) = input else {
        return None;
    };
    LABEL_KEYS.iter().find_map(|key| members.get(*key).and_then(Value::as_str)).map(one_line)
}

/// The first line of `text`, at most [`LABEL_CHARS`] characters, with ` ...` when
/// something was left out.
fn one_line(text: &str) -> String {
    let text = text.trim();
    let first = text.lines().next().unwrap_or_default();
    let mut line: String = first.chars().take(LABEL_CHARS).collect();
    if line.len() < text.len() {
        line.push_str(" ...");
    }
    line
}

/// The id of the job of a notice's text, from its first line `<job id="j3" ...>`.
fn notice_job(text: &str) -> Option<String> {
    let rest = text.strip_prefix(NOTICE_OPEN)?;
    let id = &rest[..rest.find('"')?];
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| id.to_owned())
}

/// The kind of a block of a message from `role`.
fn kind_of(role: Role, block: &ContentBlock) -> ContextPartKind {
    match (role, block) {
        (_, ContentBlock::ToolCall { .. }) => ContextPartKind::ToolCalls,
        (_, ContentBlock::ToolResult { output, .. }) if output == PRUNED_OUTPUT_STUB => {
            ContextPartKind::Pruned
        }
        (_, ContentBlock::ToolResult { .. }) => ContextPartKind::ToolOutputs,
        (Role::Assistant, _) => ContextPartKind::Answers,
        (_, ContentBlock::Text { text }) if text.starts_with(PREAMBLE_OPEN) => {
            ContextPartKind::Preambles
        }
        (_, ContentBlock::Text { text }) if text.starts_with(NOTICE_OPEN) => {
            ContextPartKind::Notices
        }
        _ => ContextPartKind::Prompts,
    }
}

/// One item of the history before it is ranked.
#[derive(Debug, Clone)]
struct Item {
    kind: ContextPartKind,
    bytes: u64,
    turn: Option<u32>,
    tool: Option<String>,
    label: Option<String>,
}

impl Item {
    fn new(kind: ContextPartKind, bytes: u64, turn: Option<u32>) -> Self {
        Item { kind, bytes, turn, tool: None, label: None }
    }
}

/// The bytes and the counts of each kind, and the items, as the request is walked.
#[derive(Debug, Default)]
struct Tally {
    bytes: HashMap<ContextPartKind, u64>,
    counts: HashMap<ContextPartKind, u32>,
    items: Vec<Item>,
}

impl Tally {
    fn add(&mut self, kind: ContextPartKind, bytes: u64, count: u32) {
        *self.bytes.entry(kind).or_default() += bytes;
        let counted = self.counts.entry(kind).or_default();
        *counted = counted.saturating_add(count);
    }

    fn item(&mut self, item: Item) {
        if item.bytes > 0 {
            self.items.push(item);
        }
    }

    /// One message of turn `turn` (its number): `original` as the window holds it, `sent` as the
    /// request sends it. The text, the reasoning and the provider items of an answer
    /// are one item; every other block is an item of its own.
    fn message(
        &mut self,
        original: &Message,
        sent: &Message,
        turn: Option<u32>,
        calls: &HashMap<String, (String, Option<String>)>,
    ) {
        let role = original.role;
        let mut answer: u64 = 0;
        let mut blocks: u64 = 0;
        let mut first = None;
        for (block, sent_block) in original.content.iter().zip(&sent.content) {
            let kind = kind_of(role, block);
            first.get_or_insert(kind);
            let bytes = json_len(sent_block);
            blocks += bytes;
            if kind == ContextPartKind::Answers {
                answer += bytes;
                self.add(kind, bytes, 0);
                continue;
            }
            self.add(kind, bytes, 1);
            let mut item = Item::new(kind, bytes, turn);
            match block {
                ContentBlock::ToolCall { name, input, .. } => {
                    item.tool = Some(name.clone());
                    item.label = call_label(name, input);
                }
                ContentBlock::ToolResult { call_id, .. } => {
                    if let Some((tool, label)) = calls.get(call_id) {
                        item.tool = Some(tool.clone());
                        item.label.clone_from(label);
                    }
                }
                ContentBlock::Text { text } if kind == ContextPartKind::Notices => {
                    item.label = notice_job(text);
                }
                _ => {}
            }
            self.item(item);
        }
        let raw = sent.provider_raw.as_ref().map_or(0, json_len);
        let whole = json_len(sent);
        let frame = whole.saturating_sub(blocks + raw);
        // NOTE: a message of only tool calls is no answer; its frame counts for them.
        if role == Role::Assistant && (answer > 0 || raw > 0 || first.is_none()) {
            answer += raw + frame;
            self.add(ContextPartKind::Answers, raw + frame, 1);
            self.item(Item::new(ContextPartKind::Answers, answer, turn));
        } else {
            self.add(first.unwrap_or(ContextPartKind::Prompts), raw + frame, 0);
        }
    }

    /// The parts and the largest items, with the tokens of the request shared out so
    /// that the parts add up to the estimate of the whole.
    fn finish(mut self) -> Measured {
        let mut parts = Vec::new();
        let mut before: u64 = 0;
        for kind in ContextPartKind::ALL {
            let bytes = self.bytes.get(&kind).copied().unwrap_or_default();
            let count = self.counts.get(&kind).copied().unwrap_or_default();
            let after = before + bytes;
            let tokens = estimate_tokens(after) - estimate_tokens(before);
            before = after;
            if bytes > 0 || count > 0 {
                parts.push(ContextPart { kind, tokens, count });
            }
        }
        // NOTE: a stable sort keeps the older of two items of one size first.
        self.items.sort_by_key(|item| std::cmp::Reverse(item.bytes));
        let largest = self
            .items
            .into_iter()
            .take(MAX_LARGEST)
            .map(|item| ContextItem {
                kind: item.kind,
                tokens: estimate_tokens(item.bytes),
                turn: item.turn,
                tool: item.tool,
                label: item.label,
            })
            .collect();
        Measured { estimate: estimate_tokens(before), parts, largest }
    }
}

/// The provider's last real count, from the newest ended turn of `page` that has one,
/// with the time of its end. `None` when a compaction or a cancelled turn came after
/// it, as for the base of the next turn's estimate (`context_base`).
pub(crate) fn counted(page: &[EventEnvelope]) -> Option<CountedUse> {
    page.iter()
        .rev()
        .find_map(|envelope| match &envelope.event {
            Event::ConversationCompacted(_) | Event::TurnCancelled { .. } => Some(None),
            Event::TurnCompleted { turn_id, context, usage, .. }
            | Event::TurnFailed { turn_id, context, usage, .. }
            | Event::TurnInterrupted { turn_id, context, usage, .. } => {
                let counted = usage.is_some_and(|usage| usage.context_tokens > 0);
                Some(context.filter(|_| counted).map(|context| CountedUse {
                    tokens: context.tokens,
                    turn_id: *turn_id,
                    at: envelope.at,
                }))
            }
            _ => None,
        })
        .flatten()
}

/// The usage of a turn's end event; `None` for any other event.
pub(crate) fn end_usage(event: &Event) -> Option<Option<Usage>> {
    match event {
        Event::TurnCompleted { usage, .. }
        | Event::TurnFailed { usage, .. }
        | Event::TurnInterrupted { usage, .. } => Some(*usage),
        _ => None,
    }
}

/// The cache use of the turns that ended with `ends` (their end events, oldest first)
/// and of `compactions`.
pub(crate) fn cache(ends: &[EventEnvelope], compactions: &[Compaction]) -> CacheSummary {
    let mut summary = CacheSummary::default();
    for usage in ends.iter().filter_map(|envelope| end_usage(&envelope.event).flatten()) {
        add(&mut summary.conversation, &usage);
        summary.turns = summary.turns.saturating_add(1);
        summary.last_turn = Some(usage);
    }
    for usage in compactions.iter().filter_map(|compaction| compaction.usage) {
        add(&mut summary.conversation, &usage);
    }
    summary
}

fn add(sum: &mut Usage, usage: &Usage) {
    sum.input_tokens = sum.input_tokens.saturating_add(usage.input_tokens);
    sum.output_tokens = sum.output_tokens.saturating_add(usage.output_tokens);
    sum.cached_input_tokens = sum.cached_input_tokens.saturating_add(usage.cached_input_tokens);
    sum.reasoning_tokens = sum.reasoning_tokens.saturating_add(usage.reasoning_tokens);
    sum.cache_write_tokens = sum.cache_write_tokens.saturating_add(usage.cache_write_tokens);
    sum.cache_write_1h_tokens =
        sum.cache_write_1h_tokens.saturating_add(usage.cache_write_1h_tokens);
}

/// The count of `compactions` (oldest first) and the facts of the newest, which efrd
/// recorded at `newest_at`.
pub(crate) fn compactions(
    compactions: &[Compaction],
    newest_at: Option<jiff::Timestamp>,
) -> CompactionSummary {
    let newest = compactions.last().zip(newest_at).map(|(compaction, at)| CompactionBrief {
        at,
        trigger: compaction.trigger,
        tokens_before: compaction.tokens_before,
        tokens_after: compaction.tokens_after,
        summarized: compaction.summary.is_some(),
        pruned_outputs: compaction.pruned_outputs,
    });
    CompactionSummary { count: u32::try_from(compactions.len()).unwrap_or(u32::MAX), newest }
}

fn json_len(value: &impl serde::Serialize) -> u64 {
    serde_json::to_vec(value).map_or(0, |json| json.len() as u64)
}

#[cfg(test)]
mod tests;
