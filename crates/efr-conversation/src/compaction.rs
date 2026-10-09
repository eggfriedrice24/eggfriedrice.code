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
use efr_stdx::time::Clock;
use futures::StreamExt as _;
use tracing::Instrument as _;

use crate::context::{
    ContextLimits, PRUNE_KEEP_TOKENS, PRUNE_MIN_TOKENS, PRUNED_OUTPUT_STUB,
    SUMMARY_MAX_OUTPUT_TOKENS, SUMMARY_REASONING_TOKENS, TAIL_TOKENS, estimate_tokens,
};
use crate::gap::{self, CallGap};
use crate::interrupt::Interrupt;
use crate::unoffered;

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
    earlier(turns, messages).map(|earlier| {
        Message::user(format!(
            "The summary leaves out {earlier}: they did not fit in the model's context."
        ))
    })
}

/// `turns` earlier turns and `messages` earlier messages in words, such as `2 earlier
/// turns and 1 earlier message`; `None` when both are 0.
fn earlier(turns: u32, messages: u32) -> Option<String> {
    let count = |n: u32, what: &str| match n {
        0 => None,
        1 => Some(format!("1 earlier {what}")),
        n => Some(format!("{n} earlier {what}s")),
    };
    let parts: Vec<String> =
        [count(turns, "turn"), count(messages, "message")].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" and "))
}

/// What a summary request leaves out of the model's history: the earlier turns that
/// the history left out (the safety net of the history limits), and the oldest
/// messages that did not fit in the request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LeftOut {
    pub(crate) turns: u32,
    pub(crate) messages: u32,
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

/// The text of the summary request's last message: the prompt, the sentence that says
/// what the request leaves out when it leaves out anything, and the user's focus when a
/// manual compaction names one.
pub(crate) fn summary_prompt(focus: Option<&str>, left_out: LeftOut) -> String {
    let mut text = SUMMARY_PROMPT.trim_end().to_owned();
    if let Some(earlier) = earlier(left_out.turns, left_out.messages) {
        let they =
            if left_out.turns.saturating_add(left_out.messages) == 1 { "it" } else { "they" };
        text.push_str(&format!(
            "\n\nThis request leaves out {earlier} of the conversation: {they} did not fit in \
             the model's context. Say so under the first heading."
        ));
    }
    if let Some(focus) = focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        text.push_str(&format!("\n\n{FOCUS}{focus}"));
    }
    text
}

/// The summary request: `base` (the model, the system prompt, the tools, the effort
/// and the options of the turn, so the request hits the prompt cache) with `messages` and the
/// summary prompt as the last message, and the summary's output limit plus room for
/// the model's reasoning, which the Responses API counts in the same limit. It is a side
/// call: its prompt and its answer never join the history. The prompt names what
/// `messages` leave out of the history (`left_out`). A call of a tool that the turn does
/// not offer shows as text, as in the turn's own requests ([`unoffered::as_offered`]).
pub(crate) fn summary_request(
    base: &Request,
    messages: Vec<Message>,
    focus: Option<&str>,
    left_out: LeftOut,
) -> Request {
    let mut messages = unoffered::as_offered(messages, &base.tools);
    messages.push(Message::user(summary_prompt(focus, left_out)));
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

/// Which messages a summary request leaves out to fit in the model's context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Trim {
    /// The newest messages that it leaves out: messages of the verbatim tail, which the
    /// new history keeps after the summary anyway. A tail message goes before any
    /// message that only the summary keeps.
    back: usize,
    /// The oldest messages after the head that it leaves out, which a note replaces;
    /// the summary loses them.
    front: usize,
}

/// The summary request of `messages` (the head first), without the messages that
/// `trim` leaves out.
fn summary_of(job: &Job<'_>, messages: &[Message], trim: Trim) -> Request {
    let head = job.window.head.len().min(messages.len());
    let end = messages.len().saturating_sub(trim.back).max(head);
    let mut sent = messages[..head].to_vec();
    if trim.front > 0 {
        sent.push(dropped_note(trim.front));
    }
    sent.extend(messages[head.saturating_add(trim.front).min(end)..end].iter().cloned());
    let left_out = LeftOut {
        turns: job.window.omitted,
        messages: u32::try_from(trim.front).unwrap_or(u32::MAX),
    };
    summary_request(job.base, sent, job.focus, left_out)
}

/// `base` with the messages of `window`, each call of a tool that `base` does not offer
/// shown as text ([`unoffered::as_offered`]).
pub(crate) fn with_window(base: &Request, window: &Window) -> Request {
    Request { messages: unoffered::as_offered(window.messages(), &base.tools), ..base.clone() }
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
    /// The start of the conversation's newest model call, which the summary call
    /// moves, and the clock that reads its start.
    pub(crate) gap: &'a CallGap,
    pub(crate) clock: &'a dyn Clock,
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
/// leaves the context at or above the trigger, the window leaves out earlier turns, or
/// the compaction is manual or follows an overflow.
pub(crate) async fn run(job: Job<'_>) -> Outcome {
    let pruning = prune(&job.window.placed);
    // NOTE: after an overflow the estimate has just counted too low, so it cannot show
    // that pruning alone makes the request fit; only a summary can. Only a summary
    // takes the place of the turns that the window leaves out, too.
    if job.trigger == CompactionTrigger::Auto && job.window.omitted == 0 && pruning.worth_it() {
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
    let too_large = overflow
        || request_tokens(&summary_of(&job, &messages, Trim::default())) > job.limits.hard_cap;
    if too_large && pruning.outputs > 0 {
        let pruned = Window {
            head: job.window.head.clone(),
            placed: pruning.placed.clone(),
            omitted: job.window.omitted,
        };
        messages = pruned.messages();
        used = Some(pruning.clone());
    }
    // NOTE: efr never sends a request above the hard cap, and a summary request at
    // least as large as the request that the provider refused is refused too, so the
    // oldest messages go before the first try. A history that grew on a model with a
    // larger window fits the window of the turn's model this way.
    let refused = overflow.then(|| request_tokens(&with_window(job.base, job.window)));
    let whole = request_tokens(&summary_of(&job, &messages, Trim::default()));
    let mut trim = Trim::default();
    if whole > job.limits.hard_cap || refused.is_some_and(|refused| whole >= refused) {
        let target = fit_target(job.limits, refused.unwrap_or(job.limits.window));
        trim = shrink(&job, &messages, tail, trim, target).unwrap_or_default();
    }
    let mut request = summary_of(&job, &messages, trim);
    let mut answer = summarize(&job, request.clone()).await;
    if matches!(&answer, Answer::Failed(error) if error.is_context_overflow()) {
        // NOTE: the summary request itself did not fit: the tail goes first, then the
        // oldest messages, never the head with an earlier summary, and the rest is
        // summarized.
        let target = fit_target(job.limits, request_tokens(&request));
        if let Some(more) = shrink(&job, &messages, tail, trim, target) {
            trim = more;
            request = summary_of(&job, &messages, trim);
            answer = summarize(&job, request).await;
        }
    }
    // NOTE: the messages after the tail's start stay word for word, also when the
    // summary request left them out; only those before it are lost to the summary.
    let omitted = u32::try_from(trim.front).unwrap_or(u32::MAX);
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

/// The estimate that a summary request must stay below after the provider refused a
/// request estimated at `refused` tokens. The provider counts more tokens than the
/// estimate, so the target is the trigger scaled by what the refusal shows: below
/// `refused` times the trigger over the window. Before any refusal, `refused` is the
/// window, so the target is the trigger, which leaves room for the answer.
fn fit_target(limits: ContextLimits, refused: u64) -> u64 {
    limits.trigger.min(refused.saturating_mul(limits.trigger) / limits.window.max(1))
}

/// The messages that the summary request must leave out, more than `trim`, to be
/// estimated below `target` tokens ([`fit_target`]). `tail` is where the verbatim tail
/// starts in the window's messages after the head. The newest messages go first, down
/// to the start of the tail, because the new history keeps them after the summary;
/// then the oldest messages after the head go. The kept messages never end with a tool
/// call, and those after the note never start with tool results, so a tool call never
/// loses its result. `None` when nothing more can go.
fn shrink(
    job: &Job<'_>,
    messages: &[Message],
    tail: usize,
    trim: Trim,
    target: u64,
) -> Option<Trim> {
    let head = job.window.head.len().min(messages.len());
    let tail_len = messages.len().saturating_sub(head.saturating_add(tail));
    let backs = (1..=tail_len)
        .filter(|&back| {
            let last = messages.len().checked_sub(back + 1);
            !last.and_then(|last| messages.get(last)).is_some_and(calls_tool)
        })
        .map(|back| Trim { back, front: 0 });
    let fronts = (1..=tail)
        .filter(|&front| front == tail || !messages.get(head + front).is_some_and(is_results))
        .map(|front| Trim { back: tail_len, front });
    let mut last = None;
    for candidate in backs.chain(fronts).filter(|candidate| *candidate > trim) {
        if request_tokens(&summary_of(job, messages, candidate)) < target {
            return Some(candidate);
        }
        last = Some(candidate);
    }
    last
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
async fn summarize(job: &Job<'_>, request: Request) -> Answer {
    let gap = job.gap.start(job.clock.now());
    let span = tracing::debug_span!(
        "provider_request",
        provider = %job.provider.id(),
        model = %request.model,
        purpose = "summary",
        gap_ms = gap.map(gap::millis),
    );
    stream_summary(job.provider, request, job.interrupt).instrument(span).await
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
