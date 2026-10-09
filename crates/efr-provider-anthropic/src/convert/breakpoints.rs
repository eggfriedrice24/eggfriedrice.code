//! Where a request puts its prompt cache markers, and for how long.
//!
//! The placement is a pure function of the shape of the request, so two requests of
//! one conversation agree on every earlier marker without any state, also across a
//! daemon restart. A marker is `"cache_control": {"type": "ephemeral", "ttl": "5m"}`
//! (or `"1h"`) on the last block of its place. There are four places at most:
//!
//! - S, the system: the system block, else the last tool. It covers the tools and the
//!   system prompt, which a pruning or a compaction does not change.
//! - A, the anchor: the last place before the tail at which an earlier call marked an
//!   anchor.
//! - P, the previous tail: the last place before the tail, where the call before put
//!   its T. The call reads that entry directly, so the API's lookback of 20 blocks
//!   never matters.
//! - T, the tail: the last message, which is a user message.
//!
//! Places. The conversion never merges user messages; the API joins a run of adjacent
//! user messages into one. A run is one place, at its last message: a new prompt after
//! a turn that ended on its tool results or on its prompt, a steer after tool results,
//! the parts of the head after a compaction, and a summary prompt after the turn's last
//! message. An inner message of a run can have ended the request of a call that failed,
//! such as the tool results before a new prompt of the next turn. No marker goes on
//! it: the API's lookback from the markers after it reads the entry of that call.
//!
//! Anchors. Every place ends the request of one call. The call that ends at a place
//! marks an anchor there when a message of its run opens a turn (it holds no
//! `tool_result` and does not follow an assistant message with tool calls, whose
//! results the request can show as text), or when the messages after the last anchor, the run included, hold
//! more than [`ANCHOR_STEP_TOKENS`]. A side call (`Request::side_call`) never marks
//! one. Each message's tokens are the estimate of the conversion, the same for the same
//! bytes, so every later request finds the same anchors. So the first call of every
//! turn marks an anchor, also after a turn that ended early, and so does a call that
//! sends a steer: it holds a person's words, and a pause can follow it.
//!
//! State. An anchor needs to know where the last anchor is and how many tokens came
//! after it. Nothing keeps these numbers: the placement computes them again from
//! [`Layout::messages`] on every call. The state is the history itself, which the
//! conversation (`efr-conversation`) keeps append-only and sends whole with every
//! request; the provider keeps nothing between calls. So the anchors survive a restart
//! of the daemon, and two requests over the same history agree.
//!
//! Times to live under [`CacheTtl::Auto`]: S and A are one hour. A call that marks an
//! anchor puts one hour on every marker, because the API refuses a one-hour marker
//! after a five-minute one; any other call puts five minutes on P and T. So a pause of
//! any length loses at most the tool loop after the last anchor. [`CacheTtl::FiveMinutes`]
//! and [`CacheTtl::OneHour`] put their one time on every marker. When two places are
//! the same message, it gets one marker with the longer time. Markers sit only on the
//! system block, a tool and the last block of a user message, so never on a `thinking`
//! block; the conversion drops empty text blocks, so never on an empty text either.
//!
//! The log. Each request writes one debug line with its setting and its markers, such
//! as `cache_ttl=auto markers=S1h,A1h,P5m,T5m` ([`summary`]). Under `auto`, `T1h` says
//! that the call marked a new anchor. The conversation's span of the call carries
//! `gap_ms`, the time since the start of the conversation's call before, so the line
//! shows which pause met which markers: the measurement of the time to live.

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

impl Slot {
    /// The letter of the place: `S`, `A`, `P` or `T`.
    const fn letter(self) -> char {
        match self {
            Slot::System => 'S',
            Slot::Anchor => 'A',
            Slot::Previous => 'P',
            Slot::Tail => 'T',
        }
    }
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

/// One message of the body, after the merge of adjacent assistant messages, as the
/// placement sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    pub(crate) role: Role,
    /// True for a user message that holds no `tool_result` and does not follow an
    /// assistant message with tool calls: the prompt that opens a turn, a steer, or a
    /// part of the head after a compaction. The results of calls that the request shows
    /// as text open no turn.
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

/// `marks` as the debug line of a request shows them: the letter and the time of each
/// place in order, such as `S1h,A1h,P5m,T5m`, or `none`.
pub(crate) fn summary(marks: &[Breakpoint]) -> String {
    if marks.is_empty() {
        return "none".to_owned();
    }
    let texts: Vec<String> =
        marks.iter().map(|mark| format!("{}{}", mark.slot.letter(), mark.ttl.as_str())).collect();
    texts.join(",")
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
    let is_user = |index: usize| messages.get(index).is_some_and(|shape| shape.role == Role::User);
    // NOTE: a run of adjacent user messages is one place: only its last message ever
    // ended a call's request, so only the last one of a run counts.
    let users: Vec<usize> =
        (0..messages.len()).filter(|&index| is_user(index) && !is_user(index + 1)).collect();
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
    // The run that ends at `index` opens a turn when one of its messages does, such as a
    // prompt after the tool results of an interrupted turn.
    let opens = |index: usize| {
        (0..=index)
            .rev()
            .take_while(|&at| is_user(at))
            .any(|at| messages.get(at).is_some_and(|shape| shape.opens_turn))
    };
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
