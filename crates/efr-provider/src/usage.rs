//! Token counts.

use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

/// The tokens one model call used, as the provider reported them.
///
/// A turn that calls tools makes several model calls; the conversation adds their
/// usages with `+` (which saturates instead of overflowing) and records the sum on the
/// wire as an `efr_protocol::Usage`.
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
}

impl Add for TokenUsage {
    type Output = TokenUsage;

    fn add(self, other: TokenUsage) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cached_input_tokens: self.cached_input_tokens.saturating_add(other.cached_input_tokens),
            reasoning_tokens: self.reasoning_tokens.saturating_add(other.reasoning_tokens),
        }
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, other: TokenUsage) {
        *self = *self + other;
    }
}

impl From<TokenUsage> for efr_protocol::Usage {
    /// The two totals the wire reports. The cached and reasoning parts are already
    /// inside them.
    fn from(usage: TokenUsage) -> Self {
        efr_protocol::Usage { input_tokens: usage.input_tokens, output_tokens: usage.output_tokens }
    }
}

#[cfg(test)]
mod tests;
