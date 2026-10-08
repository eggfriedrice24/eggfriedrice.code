//! The numbers of the context contract: the trigger, the hard cap, the estimate and the
//! budgets of a compaction. The README, section "Context", says how they are used.
//!
//! [`Meter`] keeps the context of a running turn: the last real count and the estimate
//! of the messages added after it. [`context_full`] is the failure of a turn whose
//! context does not fit.

use efr_protocol::{ContextUse, ErrorBody, ErrorCode, Event, EventEnvelope};
use efr_provider::{Message, Request};

/// The window that efr counts with for a model whose window it does not know: small
/// enough for every current model, so a compaction comes early rather than late.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

/// The default of [`CompactionConfig::auto_at`], `compaction.auto_at` in `config.toml`.
pub const DEFAULT_AUTO_AT: u32 = 76;

/// The percent of the window that no request may pass: a request estimated above it is
/// never sent; the turn compacts first, or fails when it cannot.
pub const HARD_CAP_PERCENT: u64 = 95;

/// The bytes that count as one token in the estimate before a call.
pub const BYTES_PER_TOKEN: u64 = 4;

/// The fewest tokens that a pruning must free to be worth a new cache prefix; a pruning
/// that frees less is not done.
pub const PRUNE_MIN_TOKENS: u64 = 20_000;

/// The tool outputs of the newest messages, up to this many tokens of history, are
/// never pruned.
pub const PRUNE_KEEP_TOKENS: u64 = 40_000;

/// The budget of the verbatim tail that a summary keeps after it.
pub const TAIL_TOKENS: u64 = 20_000;

/// The most tokens that the summary itself may have.
pub const SUMMARY_MAX_OUTPUT_TOKENS: u32 = 8_000;

/// The room for the model's reasoning in the summary request. The Responses API counts
/// reasoning in `max_output_tokens`, so the request's limit is
/// [`SUMMARY_MAX_OUTPUT_TOKENS`] plus this; a summary that reaches the limit is cut
/// off and fails the compaction.
pub const SUMMARY_REASONING_TOKENS: u32 = 24_000;

/// The compactions in a row, inside one turn, that may leave the context above the
/// trigger before the turn stops compacting.
pub const BREAKER_TRIES: u32 = 2;

/// What the model reads in place of a pruned tool output.
pub const PRUNED_OUTPUT_STUB: &str = "[efr removed this output to make room in the context. \
                                      The full output stays in efr's recording of the call; \
                                      run the command again if you need it.]";

/// When a conversation compacts its context on its own (`[compaction]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompactionConfig {
    /// True compacts on its own at the trigger. False compacts only on
    /// `conversation.compact`; a request above the hard cap then fails the turn.
    pub auto: bool,
    /// The trigger, in percent of the model's window, from 1 to 99.
    pub auto_at: u32,
}

impl CompactionConfig {
    /// Compaction with `auto` on or off at `auto_at` percent of the window.
    pub fn new(auto: bool, auto_at: u32) -> Self {
        CompactionConfig { auto, auto_at }
    }
}

impl Default for CompactionConfig {
    /// On, at [`DEFAULT_AUTO_AT`] percent.
    fn default() -> Self {
        CompactionConfig::new(true, DEFAULT_AUTO_AT)
    }
}

/// The token limits of one turn's model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ContextLimits {
    /// The model's window, or [`DEFAULT_CONTEXT_WINDOW`] when it is not known.
    pub window: u64,
    /// The context size at which an auto compaction runs: `auto_at` percent of the
    /// window, rounded down.
    pub trigger: u64,
    /// The largest request that may be sent: [`HARD_CAP_PERCENT`] of the window,
    /// rounded down.
    pub hard_cap: u64,
    /// Whether auto compaction is on.
    pub auto: bool,
}

impl ContextLimits {
    /// The limits of a model with `window` tokens (`None` when unknown) under `config`.
    /// An `auto_at` outside 1 to 99 counts as the nearest end.
    pub fn new(window: Option<u64>, config: CompactionConfig) -> Self {
        let window = window.filter(|window| *window > 0).unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let auto_at = u64::from(config.auto_at.clamp(1, 99));
        ContextLimits {
            window,
            trigger: percent_of(window, auto_at),
            hard_cap: percent_of(window, HARD_CAP_PERCENT),
            auto: config.auto,
        }
    }

    /// The size that a client shows as 100%: the trigger, or the hard cap when auto
    /// compaction is off.
    pub fn limit(&self) -> u64 {
        if self.auto { self.trigger } else { self.hard_cap }
    }

    /// The gauge of a context of `tokens` tokens.
    pub fn gauge(&self, tokens: u64) -> ContextUse {
        ContextUse { tokens, limit: self.limit(), window: self.window }
    }
}

/// The estimated tokens of `bytes` bytes of request JSON: one token per
/// [`BYTES_PER_TOKEN`] bytes, rounded up.
pub fn estimate_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(BYTES_PER_TOKEN)
}

fn percent_of(window: u64, percent: u64) -> u64 {
    window.saturating_mul(percent) / 100
}

/// The estimated tokens of the whole of `request`: its JSON, system prompt, tool
/// definitions and messages, at [`BYTES_PER_TOKEN`].
pub(crate) fn request_tokens(request: &Request) -> u64 {
    estimate_tokens(json_len(request))
}

/// The estimated tokens of `message`, from its JSON.
pub(crate) fn message_tokens(message: &Message) -> u64 {
    estimate_tokens(json_len(message))
}

fn json_len(value: &impl serde::Serialize) -> u64 {
    serde_json::to_vec(value).map_or(0, |json| json.len() as u64)
}

/// The context of a turn as it grows: the last real count, and the estimate of the
/// messages added after it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Meter {
    /// The last real count: the input plus the output of the last model call, or the
    /// context at the end of the previous turn. `None` when no count is known since the
    /// last compaction.
    base: Option<u64>,
    /// The estimated tokens of the messages added after `base`.
    added: u64,
}

impl Meter {
    /// A meter that starts from `base`, the context at the end of the previous turn.
    pub(crate) fn new(base: Option<u64>) -> Self {
        Meter { base, added: 0 }
    }

    /// `message` joins the context.
    pub(crate) fn add(&mut self, message: &Message) {
        self.added = self.added.saturating_add(message_tokens(message));
    }

    /// A model call reported `tokens` as its input plus its output: the new base.
    pub(crate) fn counted(&mut self, tokens: u64) {
        self.base = Some(tokens);
        self.added = 0;
    }

    /// A compaction changed the history: no count is known until the next call.
    pub(crate) fn reset(&mut self) {
        *self = Meter::default();
    }

    /// The estimated context of `request`: the base plus what came after it, or
    /// without a base the estimate of the whole request.
    pub(crate) fn estimate(&self, request: &Request) -> u64 {
        self.known().unwrap_or_else(|| request_tokens(request))
    }

    /// The base plus what came after it; `None` without a base.
    pub(crate) fn known(&self) -> Option<u64> {
        self.base.map(|base| base.saturating_add(self.added))
    }
}

/// The context at the end of the newest turn of `page`, the base of the next turn's
/// estimate. `None` when a compaction came after it, when the newest turn was
/// cancelled or has no end (its messages are in the history, but in no count), or when
/// the newest ended turn has no `context` (a turn from before efr counted it, or one
/// that ended before it sent anything) or no real count (no call of it reported its
/// usage): its messages are then not in any count, and an estimate is no base.
pub(crate) fn context_base(page: &[EventEnvelope]) -> Option<u64> {
    page.iter().rev().find_map(|envelope| match &envelope.event {
        Event::ConversationCompacted(_)
        | Event::TurnCancelled { .. }
        | Event::TurnStarted { .. } => Some(None),
        Event::TurnCompleted { context, usage, .. }
        | Event::TurnFailed { context, usage, .. }
        | Event::TurnInterrupted { context, usage, .. } => {
            let counted = usage.is_some_and(|usage| usage.context_tokens > 0);
            Some(context.filter(|_| counted).map(|context| context.tokens))
        }
        _ => None,
    })?
}

/// Why a turn's context does not fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Full {
    /// The next request is estimated above the hard cap, and the turn cannot compact.
    Cap,
    /// The provider refused the request, and auto compaction is off.
    Refused,
    /// The compaction to make room failed: after the provider refused the request
    /// (`refused`), or before a request above the hard cap. `failure` is why the
    /// summary failed, in one sentence; `None` when nothing lay before the tail.
    CompactionFailed { refused: bool, failure: Option<String> },
    /// The compactions of this turn did not free enough room.
    Breaker,
    /// The provider refused the request again after a compaction.
    StillRefused,
}

/// The `turn_failed` body of a turn whose context of `tokens` tokens does not fit:
/// code `internal`, data `{"cause": "context_overflow", "tokens", "window"}`, and a
/// message that names the cause and the way out.
pub(crate) fn context_full(why: Full, tokens: u64, limits: &ContextLimits) -> ErrorBody {
    let (size, window, cap) = (kilo(tokens), kilo(limits.window), kilo(limits.hard_cap));
    let message = match why {
        Full::Cap => {
            format!("the context is full: about {size} of {window} tokens, above the cap of {cap}")
        }
        Full::Refused => {
            format!("the context is full: the model refused about {size} of {window} tokens")
        }
        Full::CompactionFailed { refused, failure } => {
            let failed = match failure {
                Some(failure) => {
                    format!("the context is full and the compaction failed ({failure})")
                }
                None => "the context is full and the compaction failed".to_owned(),
            };
            if refused {
                format!("{failed}: the model refused about {size} of {window} tokens")
            } else {
                format!("{failed}: about {size} of {window} tokens, above the cap of {cap}")
            }
        }
        Full::Breaker => format!(
            "the context is full: compaction did not free enough room (still about {size} of {window} tokens)"
        ),
        Full::StillRefused => format!(
            "the context is still full after a compaction: the model refused about {size} of {window} tokens"
        ),
    };
    ErrorBody::new(
        ErrorCode::Internal,
        format!("{message}; run ,compact or start a new conversation"),
    )
    .with_data(serde_json::json!({
        "cause": CONTEXT_OVERFLOW,
        "tokens": tokens,
        "window": limits.window,
    }))
}

/// The `cause` of a `turn_failed` whose context did not fit.
pub(crate) const CONTEXT_OVERFLOW: &str = "context_overflow";

/// `tokens` as a person reads it: `950`, `3.2k`, `281k`.
pub(crate) fn kilo(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..10_000 => {
            let tenths = (tokens + 50) / 100;
            format!("{}.{}k", tenths / 10, tenths % 10)
        }
        _ => format!("{}k", (tokens + 500) / 1_000),
    }
}

#[cfg(test)]
mod tests;
