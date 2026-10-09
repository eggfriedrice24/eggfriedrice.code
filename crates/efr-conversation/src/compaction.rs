//! The compaction of a conversation's context: the model's history as a [`Window`],
//! the pure steps (prune, cut, the summary request) and the summary call.
//!
//! The README, section "Context", is the contract. A turn runs a compaction between two
//! model calls (`turn/compact.rs`), and the actor runs a manual one between turns. Both
//! hand a [`Job`] to [`run`], which prunes, and summarizes when pruning is not enough or
//! not allowed. The caller then reads the fresh context block from disk, builds the new
//! window and records `conversation_compacted`, which stores the block.

use std::collections::HashSet;

use efr_protocol::{CompactionTrigger, TurnId};
use efr_provider::{
    CompletionBuilder, ContentBlock, Message, Provider, ProviderError, Request, Role, StopReason,
    TokenUsage,
};
use futures::StreamExt as _;
use tracing::Instrument as _;

use crate::context::{
    ContextLimits, PRUNE_KEEP_TOKENS, PRUNE_MIN_TOKENS, PRUNED_OUTPUT_STUB,
    SUMMARY_MAX_OUTPUT_TOKENS, SUMMARY_REASONING_TOKENS, TAIL_TOKENS, estimate_tokens,
};
use crate::interrupt::Interrupt;

/// The summary prompt: the one text that every compaction sends, and the format of the
/// handoff record of milestone 4.
pub(crate) const SUMMARY_PROMPT: &str = include_str!("compaction/prompt.md");

/// What the summary message starts with, so the model tells it from a prompt.
pub(crate) const SUMMARY_OPEN: &str = "<conversation-summary>";

/// What the summary message ends with.
pub(crate) const SUMMARY_CLOSE: &str = "</conversation-summary>";

/// What comes before the user's focus text in the summary request.
const FOCUS: &str = "Keep in the summary: ";

/// One message of the model's history and where it stands: message `index` of the turn
/// `turn` (0 is the prompt).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Placed {
    pub(crate) turn: TurnId,
    pub(crate) index: u32,
    pub(crate) message: Message,
}

/// The model's history as the next request sends it, after the system prompt.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Window {
    /// The fresh context block and the summary of the newest compaction; empty before
    /// the first compaction with a summary.
    pub(crate) head: Vec<Message>,
    /// The messages of the turns, each with its place, oldest first.
    pub(crate) placed: Vec<Placed>,
    /// The earlier turns that the history leaves out and that no summary covers (the
    /// safety net of the history limits); the head says so to the model.
    pub(crate) omitted: u32,
}

impl Window {
    /// The messages of the request, in order.
    pub(crate) fn messages(&self) -> Vec<Message> {
        self.head
            .iter()
            .cloned()
            .chain(self.placed.iter().map(|placed| placed.message.clone()))
            .collect()
    }
}

/// A place in the history: the compaction covers every message before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cut {
    /// The newest turn whose messages the compaction covers, in full or in part.
    pub(crate) through_turn: TurnId,
    /// The first message of `through_turn` after the cut; `None` when the compaction
    /// covers the whole turn.
    pub(crate) through_message: Option<u32>,
}

impl Cut {
    /// True when `placed` lies before the cut. `order` gives the position of a turn in
    /// the conversation; a turn it does not know counts as after the cut.
    pub(crate) fn covers(&self, placed: &Placed, order: impl Fn(TurnId) -> Option<usize>) -> bool {
        if placed.turn == self.through_turn {
            return self.through_message.is_none_or(|first| placed.index < first);
        }
        match (order(placed.turn), order(self.through_turn)) {
            (Some(turn), Some(through)) => turn < through,
            _ => false,
        }
    }
}

/// What pruning made of a window's turns.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pruning {
    /// The messages with the stub in each pruned tool result.
    pub(crate) placed: Vec<Placed>,
    /// The tool results that read the stub now.
    pub(crate) outputs: u32,
    /// The estimated tokens that the stubs freed.
    pub(crate) tokens: u64,
    /// The place after the newest pruned result; `None` when nothing was pruned.
    pub(crate) cut: Option<Cut>,
    /// The position in `placed` of the first message after the cut; 0 when nothing was
    /// pruned.
    pub(crate) after: usize,
}

impl Pruning {
    /// True when the pruning frees enough to be worth a new cache prefix.
    pub(crate) fn worth_it(&self) -> bool {
        self.tokens >= PRUNE_MIN_TOKENS
    }
}

pub(crate) use crate::context::{message_tokens, request_tokens};

fn json_len<T: serde::Serialize>(value: &T) -> u64 {
    serde_json::to_vec(value).map_or(0, |json| json.len() as u64)
}

/// True for a user message that holds only tool results.
pub(crate) fn is_results(message: &Message) -> bool {
    message.role == Role::User
        && !message.content.is_empty()
        && message.content.iter().all(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

/// True for an assistant message that calls a tool.
fn calls_tool(message: &Message) -> bool {
    message.role == Role::Assistant
        && message.content.iter().any(|block| matches!(block, ContentBlock::ToolCall { .. }))
}

/// Where the shortest tail starts: the newest tool call with its result, or the newest
/// prompt (or steer), whichever is newer. Nothing in it is pruned or summarized away.
pub(crate) fn minimal_tail(placed: &[Placed]) -> usize {
    placed
        .iter()
        .rposition(|placed| {
            calls_tool(&placed.message)
                || (placed.message.role == Role::User && !is_results(&placed.message))
        })
        .unwrap_or(placed.len().saturating_sub(1))
}

/// Gives every tool result outside the newest [`PRUNE_KEEP_TOKENS`] of `placed`, and
/// outside the shortest tail, whose output is longer than [`PRUNED_OUTPUT_STUB`] the
/// stub as its output. The caller decides with [`Pruning::worth_it`] whether to use it.
pub(crate) fn prune(placed: &[Placed]) -> Pruning {
    let mut kept: u64 = 0;
    let mut protected = placed.len();
    for (at, item) in placed.iter().enumerate().rev() {
        kept += message_tokens(&item.message);
        if kept > PRUNE_KEEP_TOKENS {
            break;
        }
        protected = at;
    }
    let protected = protected.min(minimal_tail(placed));
    let mut pruned = placed.to_vec();
    let mut outputs = 0;
    let mut freed_bytes: u64 = 0;
    let mut newest = None;
    for (at, item) in pruned.iter_mut().enumerate().take(protected) {
        let before = json_len(&item.message);
        let mut stubbed = false;
        for block in &mut item.message.content {
            if let ContentBlock::ToolResult { output, .. } = block
                && output.len() > PRUNED_OUTPUT_STUB.len()
            {
                PRUNED_OUTPUT_STUB.clone_into(output);
                outputs += 1;
                stubbed = true;
            }
        }
        if stubbed {
            freed_bytes += before.saturating_sub(json_len(&item.message));
            newest = Some(at);
        }
    }
    let cut = newest.map(|at| place_after(&pruned, at));
    let after = newest.map_or(0, |at| at + 1);
    Pruning { placed: pruned, outputs, tokens: estimate_tokens(freed_bytes), cut, after }
}

/// The place right after `placed[at]`.
fn place_after(placed: &[Placed], at: usize) -> Cut {
    let item = &placed[at];
    let whole_turn = placed.get(at + 1).is_some_and(|next| next.turn != item.turn);
    Cut {
        through_turn: item.turn,
        through_message: (!whole_turn).then_some(item.index.saturating_add(1)),
    }
}

/// Where the verbatim tail of `placed` starts: the newest messages whose estimate fits
/// in [`TAIL_TOKENS`], at least the [`minimal_tail`], and never at a message of tool
/// results, so a tool call keeps its result. 0 means that nothing lies before the tail.
pub(crate) fn tail_start(placed: &[Placed]) -> usize {
    let mut kept: u64 = 0;
    let mut start = placed.len();
    for (at, item) in placed.iter().enumerate().rev() {
        kept += message_tokens(&item.message);
        if kept > TAIL_TOKENS {
            break;
        }
        start = at;
    }
    let mut start = start.min(minimal_tail(placed));
    while start > 0 && placed.get(start).is_some_and(|item| is_results(&item.message)) {
        start -= 1;
    }
    start
}

/// The cut before `placed[start]`, or `None` when `start` is 0 and nothing lies before
/// it.
pub(crate) fn cut_before(placed: &[Placed], start: usize) -> Option<Cut> {
    let first = placed.get(start)?;
    if start == 0 {
        return None;
    }
    if first.index > 0 {
        return Some(Cut { through_turn: first.turn, through_message: Some(first.index) });
    }
    placed.get(start - 1).map(|before| Cut { through_turn: before.turn, through_message: None })
}

/// How many turns `placed` holds messages of.
pub(crate) fn turn_count(placed: &[Placed]) -> u32 {
    let turns: HashSet<TurnId> = placed.iter().map(|placed| placed.turn).collect();
    u32::try_from(turns.len()).unwrap_or(u32::MAX)
}

/// The user message that carries `summary` in the history after a compaction.
pub(crate) fn summary_message(summary: &str) -> Message {
    Message::user(format!("{SUMMARY_OPEN}\n{}\n{SUMMARY_CLOSE}", summary.trim()))
}

/// The user message after the summary that says what the summary never saw: `turns`
/// earlier turns that the history had left out, and `messages` of the oldest messages
/// that did not fit in the summary request. `None` when it saw everything.
pub(crate) fn gap_note(turns: u32, messages: u32) -> Option<Message> {
    let count = |n: u32, what: &str| match n {
        0 => None,
        1 => Some(format!("1 earlier {what}")),
        n => Some(format!("{n} earlier {what}s")),
    };
    let parts: Vec<String> =
        [count(turns, "turn"), count(messages, "message")].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| {
        Message::user(format!(
            "The summary leaves out {}: they did not fit in the model's context.",
            parts.join(" and ")
        ))
    })
}

/// What the summary request holds in place of the `dropped` oldest messages that did
/// not fit in it.
pub(crate) fn dropped_note(dropped: usize) -> Message {
    if dropped == 1 {
        Message::user("1 earlier message is omitted: it did not fit in this request.")
    } else {
        Message::user(format!(
            "{dropped} earlier messages are omitted: they did not fit in this request."
        ))
    }
}

/// The text of the summary request's last message: the prompt, and the user's focus
/// when a manual compaction names one.
pub(crate) fn summary_prompt(focus: Option<&str>) -> String {
    match focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        Some(focus) => format!("{}\n\n{FOCUS}{focus}", SUMMARY_PROMPT.trim_end()),
        None => SUMMARY_PROMPT.trim_end().to_owned(),
    }
}

/// The summary request: `base` (the model, the system prompt, the tools, the effort
/// and the options of the turn, so the request hits the prompt cache) with `messages` and the
/// summary prompt as the last message, and the summary's output limit plus room for
/// the model's reasoning, which the Responses API counts in the same limit. It is a side
/// call: its prompt and its answer never join the history.
pub(crate) fn summary_request(
    base: &Request,
    messages: Vec<Message>,
    focus: Option<&str>,
) -> Request {
    let mut messages = messages;
    messages.push(Message::user(summary_prompt(focus)));
    Request {
        model: base.model.clone(),
        system: base.system.clone(),
        messages,
        tools: base.tools.clone(),
        max_output_tokens: Some(SUMMARY_MAX_OUTPUT_TOKENS + SUMMARY_REASONING_TOKENS),
        effort: base.effort.clone(),
        side_call: true,
        provider_options: base.provider_options.clone(),
    }
}

/// The summary request of `messages` (the head first), without the `dropped` oldest
/// messages after the head, which a note replaces.
fn summary_of(job: &Job<'_>, messages: &[Message], dropped: usize) -> Request {
    let head = job.window.head.len().min(messages.len());
    let mut sent = messages[..head].to_vec();
    if dropped > 0 {
        sent.push(dropped_note(dropped));
    }
    sent.extend(messages[head.saturating_add(dropped).min(messages.len())..].iter().cloned());
    summary_request(job.base, sent, job.focus)
}

/// `base` with the messages of `window`.
pub(crate) fn with_window(base: &Request, window: &Window) -> Request {
    Request { messages: window.messages(), ..base.clone() }
}

/// One compaction to run.
pub(crate) struct Job<'a> {
    pub(crate) provider: &'a dyn Provider,
    /// The model, the system prompt, the tools and the options of the requests; its
    /// messages do not count.
    pub(crate) base: &'a Request,
    pub(crate) window: &'a Window,
    pub(crate) limits: ContextLimits,
    pub(crate) trigger: CompactionTrigger,
    pub(crate) focus: Option<&'a str>,
    /// Stops the summary call when it is raised; a manual compaction has none.
    pub(crate) interrupt: Option<&'a Interrupt>,
}

/// What [`run`] did.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Pruning alone brought the context below the trigger.
    Pruned(Pruning),
    /// The model wrote a summary of the history; the verbatim tail starts at
    /// `window.placed[tail]`.
    Summarized {
        summary: String,
        usage: Option<TokenUsage>,
        tail: usize,
        /// The pruning that the summary request used, when it needed one to fit.
        pruned: Option<Pruning>,
        /// The oldest messages before the cut that the summary request left out because
        /// they did not fit in the model's context.
        omitted: u32,
    },
    /// Nothing lies before the shortest tail, so a compaction frees no room.
    Nothing,
    /// The summary request failed.
    Failed(ProviderError),
    /// The model answered the summary request without text.
    Empty,
    /// The model stopped the summary before its end: it reached the output limit, or
    /// the provider stopped it for its content.
    Incomplete(StopReason),
    /// The user interrupted the turn during the summary call.
    Interrupted,
}

/// Runs the steps of one compaction: prune, and summarize when pruning frees too little,
/// leaves the context at or above the trigger, or the compaction is manual or follows
/// an overflow.
pub(crate) async fn run(job: Job<'_>) -> Outcome {
    let pruning = prune(&job.window.placed);
    // NOTE: after an overflow the estimate has just counted too low, so it cannot show
    // that pruning alone makes the request fit; only a summary can.
    if job.trigger == CompactionTrigger::Auto && pruning.worth_it() {
        let pruned = Window {
            head: job.window.head.clone(),
            placed: pruning.placed.clone(),
            omitted: job.window.omitted,
        };
        if request_tokens(&with_window(job.base, &pruned)) < job.limits.trigger {
            return Outcome::Pruned(pruning);
        }
    }
    let tail = tail_start(&job.window.placed);
    if tail == 0 {
        return Outcome::Nothing;
    }
    // NOTE: the history as the last request sent it hits the prompt cache. Only a
    // request that would not fit sends the pruned copy; a refused request did not fit.
    let overflow = job.trigger == CompactionTrigger::Overflow;
    let mut messages = job.window.messages();
    let mut used = None;
    let too_large =
        overflow || request_tokens(&summary_of(&job, &messages, 0)) > job.limits.hard_cap;
    if too_large && pruning.outputs > 0 {
        let pruned = Window {
            head: job.window.head.clone(),
            placed: pruning.placed.clone(),
            omitted: job.window.omitted,
        };
        messages = pruned.messages();
        used = Some(pruning.clone());
    }
    let mut dropped = 0;
    if overflow {
        // NOTE: a summary request at least as large as the request that the provider
        // refused is refused too, so its oldest messages go before the first try.
        let refused = request_tokens(&with_window(job.base, job.window));
        if request_tokens(&summary_of(&job, &messages, 0)) >= refused {
            dropped = shrink(&job, &messages, 0, refused).unwrap_or(0);
        }
    }
    let mut request = summary_of(&job, &messages, dropped);
    let mut answer = summarize(job.provider, request.clone(), job.interrupt).await;
    if matches!(&answer, Answer::Failed(error) if error.is_context_overflow()) {
        // NOTE: the summary request itself did not fit: the oldest messages go, never
        // the head with an earlier summary, and the rest is summarized.
        if let Some(fewer) = shrink(&job, &messages, dropped, request_tokens(&request)) {
            dropped = fewer;
            request = summary_of(&job, &messages, dropped);
            answer = summarize(job.provider, request, job.interrupt).await;
        }
    }
    // NOTE: the messages after the tail's start stay word for word; only those before
    // it are lost to the summary.
    let omitted = u32::try_from(dropped.min(tail)).unwrap_or(u32::MAX);
    match answer {
        Answer::Done {
            stop: stop @ (StopReason::MaxTokens | StopReason::ContentFilter), ..
        } => Outcome::Incomplete(stop),
        Answer::Done { text, .. } if text.trim().is_empty() => Outcome::Empty,
        Answer::Done { text, usage, .. } => {
            if omitted > 0 {
                tracing::warn!(
                    omitted,
                    "the summary leaves out the oldest messages: they did not fit in the model's context"
                );
            }
            Outcome::Summarized {
                summary: text.trim().to_owned(),
                usage,
                tail,
                pruned: used,
                omitted,
            }
        }
        Answer::Failed(error) => Outcome::Failed(error),
        Answer::Interrupted => Outcome::Interrupted,
    }
}

/// How many of the oldest messages after the head of `messages` the summary request
/// must leave out, more than `dropped`, to fit after the provider refused a request
/// estimated at `refused` tokens. The provider counts more tokens than the estimate,
/// so the target is the trigger scaled by what the refusal shows: below `refused`
/// times the trigger over the window. The messages after the note never start with tool
/// results. `None` when nothing more can go.
fn shrink(job: &Job<'_>, messages: &[Message], dropped: usize, refused: u64) -> Option<usize> {
    let limits = job.limits;
    let target = limits.trigger.min(refused.saturating_mul(limits.trigger) / limits.window.max(1));
    let head = job.window.head.len().min(messages.len());
    let movable = messages.len() - head;
    for drop in dropped.saturating_add(1)..=movable {
        if messages.get(head + drop).is_some_and(is_results) {
            continue;
        }
        if drop == movable || request_tokens(&summary_of(job, messages, drop)) < target {
            return Some(drop);
        }
    }
    None
}

/// How the summary call ended.
enum Answer {
    Done { text: String, usage: Option<TokenUsage>, stop: StopReason },
    Failed(ProviderError),
    Interrupted,
}

/// Sends the summary request and keeps the text of the answer. Nothing is recorded: the
/// summary reaches the log only in `conversation_compacted`. Tool calls in the answer
/// are ignored.
async fn summarize(
    provider: &dyn Provider,
    request: Request,
    interrupt: Option<&Interrupt>,
) -> Answer {
    let span = tracing::debug_span!(
        "provider_request",
        provider = %provider.id(),
        model = %request.model,
        purpose = "summary",
    );
    stream_summary(provider, request, interrupt).instrument(span).await
}

async fn stream_summary(
    provider: &dyn Provider,
    request: Request,
    interrupt: Option<&Interrupt>,
) -> Answer {
    let raised = async {
        match interrupt {
            Some(interrupt) => interrupt.raised().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(raised);
    let opened = tokio::select! {
        biased;
        () = &mut raised => return Answer::Interrupted,
        opened = provider.stream(request) => opened,
    };
    let mut stream = match opened {
        Ok(stream) => stream,
        Err(error) => return Answer::Failed(error),
    };
    let mut builder = CompletionBuilder::new();
    loop {
        tokio::select! {
            biased;
            () = &mut raised => return Answer::Interrupted,
            item = stream.next() => match item {
                None => break,
                Some(Err(error)) => return Answer::Failed(error),
                Some(Ok(event)) => {
                    if let Err(error) = builder.push(&event) {
                        return Answer::Failed(error);
                    }
                }
            },
        }
    }
    match builder.finish() {
        Ok(completion) => Answer::Done {
            text: completion.message.text(),
            usage: completion.usage,
            stop: completion.stop_reason,
        },
        Err(error) => Answer::Failed(error),
    }
}

#[cfg(test)]
mod tests;
