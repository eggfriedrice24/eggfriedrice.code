//! Where a request puts its prompt cache markers, and for how long.
//!
//! The placement is a pure function of the shape of the request, so two requests of
//! one conversation agree on every earlier marker without any state, also across a
//! daemon restart. A marker is `"cache_control": {"type": "ephemeral", "ttl": "5m"}`
//! (or `"1h"`) on the last block of its place. There are four places at most:
//!
//! - S, the system: the system block, else the last tool. It covers the tools and the
//!   system prompt, which a pruning or a compaction does not change.
//! - A, the anchor: the last user message before the tail at which an earlier call
//!   marked an anchor.
//! - P, the previous tail: the last user message before the tail, where the call
//!   before put its T. The call reads that entry directly, so the API's lookback of 20
//!   blocks never matters.
//! - T, the tail: the last message, which is a user message.
//!
//! Anchors. Every user message ends the request of one call. The call that ends at a
//! user message marks an anchor there when the message opens a turn (it holds no
//! `tool_result`), or when the messages after the last anchor, that one included, hold
//! more than [`ANCHOR_STEP_TOKENS`]. A side call (`Request::side_call`) never marks one.
//! Each message's tokens are the estimate of the conversion, the same for the same
//! bytes, so every later request finds the same anchors.
//!
//! Times to live under [`CacheTtl::Auto`]: S and A are one hour. A call that marks an
//! anchor puts one hour on every marker, because the API refuses a one-hour marker
//! after a five-minute one; any other call puts five minutes on P and T. So a pause of
//! any length loses at most the tool loop after the last anchor. [`CacheTtl::FiveMinutes`]
//! and [`CacheTtl::OneHour`] put their one time on every marker. When two places are
//! the same message, it gets one marker with the longer time. Markers sit only on the
//! system block, a tool and user messages, never on a `thinking` block.
//!
//! A known gap: a new prompt after an interrupted call merges into the user message of
//! the call's results, so that call does not open a turn here; it marks an anchor only
//! by size.

use efr_provider::Role;

use crate::CacheTtl;

/// About how many tokens a tool loop may add after the last anchor before a call marks
/// a new one: what a pause that lets the five-minute entries expire loses at most.
pub(crate) const ANCHOR_STEP_TOKENS: u64 = 20_000;

/// How long one marker keeps its entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ttl {
    /// `"5m"`.
    FiveMinutes,
    /// `"1h"`.
    OneHour,
}

impl Ttl {
    /// The value of the marker's `ttl` member.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Ttl::FiveMinutes => "5m",
            Ttl::OneHour => "1h",
        }
    }
}

/// Which of the four places a marker is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// S, the system prompt or the last tool.
    System,
    /// A, the last anchor before the tail.
    Anchor,
    /// P, the tail of the call before.
    Previous,
    /// T, the last message.
    Tail,
}

/// The block that a marker goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// The system block.
    System,
    /// The last tool.
    LastTool,
    /// The last block of the message at this index of the body's `messages`.
    Message(usize),
}

/// One marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Breakpoint {
    pub(crate) slot: Slot,
    pub(crate) target: Target,
    pub(crate) ttl: Ttl,
}

/// One message of the body, after the merge, as the placement sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    pub(crate) role: Role,
    /// True for a user message that holds no `tool_result`: the prompt that opens a
    /// turn, or the head after a compaction.
    pub(crate) opens_turn: bool,
    /// The conversion's estimate of the message's tokens.
    pub(crate) tokens: u64,
}

/// A request as the placement sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Layout<'a> {
    /// True when the body has a system block.
    pub(crate) system: bool,
    /// True when the body has at least one tool.
    pub(crate) tools: bool,
    /// The body's messages, in order.
    pub(crate) messages: &'a [Shape],
    /// `Request::side_call`.
    pub(crate) side_call: bool,
}

/// The markers of the request that `layout` describes under `ttl`, at most four, in
/// the order of the prefix: S, then the messages from the oldest.
pub(crate) fn place_breakpoints(layout: &Layout<'_>, ttl: CacheTtl) -> Vec<Breakpoint> {
    let (long, short) = match ttl {
        CacheTtl::FiveMinutes => (Ttl::FiveMinutes, Ttl::FiveMinutes),
        CacheTtl::OneHour => (Ttl::OneHour, Ttl::OneHour),
        CacheTtl::Auto => (Ttl::OneHour, Ttl::FiveMinutes),
    };
    let mut marks = Vec::with_capacity(4);
    let system = match (layout.system, layout.tools) {
        (true, _) => Some(Target::System),
        (false, true) => Some(Target::LastTool),
        (false, false) => None,
    };
    if let Some(target) = system {
        marks.push(Breakpoint { slot: Slot::System, target, ttl: long });
    }

    let messages = layout.messages;
    let users: Vec<usize> = (0..messages.len())
        .filter(|&index| messages.get(index).is_some_and(|shape| shape.role == Role::User))
        .collect();
    let Some((&tail, past)) = users.split_last() else {
        return marks;
    };
    if tail + 1 != messages.len() {
        // NOTE: a body that ends with an assistant message has no tail to mark; the
        // API refuses it anyway.
        return marks;
    }

    let tokens = |from: usize, to: usize| -> u64 {
        messages
            .get(from..=to)
            .map_or(0, |run| run.iter().fold(0u64, |sum, shape| sum.saturating_add(shape.tokens)))
    };
    let opens = |index: usize| messages.get(index).is_some_and(|shape| shape.opens_turn);
    let mut anchor = None;
    let mut since = 0u64;
    let mut from = 0;
    for &index in past {
        since = since.saturating_add(tokens(from, index));
        from = index + 1;
        if opens(index) || since > ANCHOR_STEP_TOKENS {
            anchor = Some(index);
            since = 0;
        }
    }
    since = since.saturating_add(tokens(from, tail));
    let tail_anchors = !layout.side_call && (opens(tail) || since > ANCHOR_STEP_TOKENS);
    let loop_ttl = if tail_anchors { long } else { short };

    let previous = past.last().copied();
    if let Some(index) = anchor {
        marks.push(Breakpoint { slot: Slot::Anchor, target: Target::Message(index), ttl: long });
    }
    if let Some(index) = previous.filter(|&index| Some(index) != anchor) {
        marks.push(Breakpoint {
            slot: Slot::Previous,
            target: Target::Message(index),
            ttl: loop_ttl,
        });
    }
    marks.push(Breakpoint { slot: Slot::Tail, target: Target::Message(tail), ttl: loop_ttl });
    marks
}

#[cfg(test)]
mod tests;
