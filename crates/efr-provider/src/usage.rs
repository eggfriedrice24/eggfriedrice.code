//! Token counts.

use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

/// The tokens one model call used, as the provider reported them.
///
/// A turn that calls tools makes several model calls; the conversation adds their
/// usages with `+` (which saturates instead of overflowing) and records the sum on the
/// wire as an `efr_protocol::Usage`.
///
/// The counts have one meaning for every provider. `input_tokens` is the whole input
/// of the call; the cache counts are parts of it, never added to it. A provider whose
/// API reports the parts apart adds them: for Anthropic's Messages API,
/// `input_tokens` is its `input_tokens + cache_creation_input_tokens +
/// cache_read_input_tokens`. The context gauge and the compaction trigger read
/// `input_tokens`, so a provider that leaves out a part makes them read too low.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens sent to the model, cached ones included.
    pub input_tokens: u64,
    /// Tokens the model produced, reasoning included.
    pub output_tokens: u64,
    /// The part of `input_tokens` the provider served from its prompt cache.
    #[serde(default)]
    pub cached_input_tokens: u64,
    /// The part of `output_tokens` the model spent on reasoning.
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// The part of `input_tokens` the provider wrote to its prompt cache, with every
    /// time to live.
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// The part of `cache_write_tokens` written with a time to live of one hour. Zero
    /// for a provider without such a choice, such as OpenAI.
    #[serde(default)]
    pub cache_write_1h_tokens: u64,
}

impl Add for TokenUsage {
    type Output = TokenUsage;

    fn add(self, other: TokenUsage) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cached_input_tokens: self.cached_input_tokens.saturating_add(other.cached_input_tokens),
            reasoning_tokens: self.reasoning_tokens.saturating_add(other.reasoning_tokens),
            cache_write_tokens: self.cache_write_tokens.saturating_add(other.cache_write_tokens),
            cache_write_1h_tokens: self
                .cache_write_1h_tokens
                .saturating_add(other.cache_write_1h_tokens),
        }
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, other: TokenUsage) {
        *self = *self + other;
    }
}

impl From<TokenUsage> for efr_protocol::Usage {
    /// The counts of `usage` as the wire reports them. `context_tokens` stays zero: a
    /// sum of calls does not know the size of the last one, so the turn sets it.
    fn from(usage: TokenUsage) -> Self {
        efr_protocol::Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            cache_write_1h_tokens: usage.cache_write_1h_tokens,
            context_tokens: 0,
        }
    }
}

#[cfg(test)]
mod tests;
