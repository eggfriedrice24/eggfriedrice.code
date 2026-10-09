//! Anthropic's usage counts in the canonical sense of `efr_provider::TokenUsage`.
//!
//! The Messages API reports the input in three parts that do not overlap:
//! `input_tokens` (only the part after the last cache marker that hit or wrote),
//! `cache_creation_input_tokens` (written to the cache) and `cache_read_input_tokens`
//! (read from it). efr's `input_tokens` is the whole input, so it is the sum of the
//! three, and the cache counts are parts of it:
//!
//! | Canonical | Anthropic |
//! |---|---|
//! | `input_tokens` | `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` |
//! | `cached_input_tokens` | `cache_read_input_tokens` |
//! | `cache_write_tokens` | `cache_creation_input_tokens` |
//! | `cache_write_1h_tokens` | `cache_creation.ephemeral_1h_input_tokens` |
//! | `output_tokens` | `output_tokens` |
//! | `reasoning_tokens` | `output_tokens_details.thinking_tokens` |
//!
//! Without the sum, the context gauge of a warm cache reads about 0% and the
//! compaction never runs. The stream gives the usage in `message_start` and updates it
//! in each `message_delta` with cumulative counts, so the stream mapper keeps one usage
//! object, overwrites each member that an event sends, and converts it once at the end.

use efr_provider::TokenUsage;
use serde_json::Value;

/// The canonical counts of `usage`, a usage object of the Messages API after the last
/// `message_delta`. A missing or `null` count is zero.
#[cfg_attr(not(test), expect(dead_code, reason = "the stream mapper is not built yet"))]
pub(crate) fn token_usage(usage: &Value) -> TokenUsage {
    let count = |path: &[&str]| {
        path.iter().try_fold(usage, |at, key| at.get(key)).and_then(Value::as_u64).unwrap_or(0)
    };
    let written = count(&["cache_creation_input_tokens"]);
    let read = count(&["cache_read_input_tokens"]);
    TokenUsage {
        input_tokens: count(&["input_tokens"]).saturating_add(written).saturating_add(read),
        output_tokens: count(&["output_tokens"]),
        cached_input_tokens: read,
        reasoning_tokens: count(&["output_tokens_details", "thinking_tokens"]),
        cache_write_tokens: written,
        cache_write_1h_tokens: count(&["cache_creation", "ephemeral_1h_input_tokens"]),
    }
}

#[cfg(test)]
mod tests;
