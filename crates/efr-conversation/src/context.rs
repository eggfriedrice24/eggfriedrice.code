//! The numbers of the context contract: the trigger, the hard cap, the estimate and the
//! budgets of a compaction. The README, section "Context", says how they are used.

use efr_protocol::ContextUse;

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

/// The most tokens that the summary request may produce.
pub const SUMMARY_MAX_OUTPUT_TOKENS: u32 = 8_000;

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

#[cfg(test)]
mod tests;
