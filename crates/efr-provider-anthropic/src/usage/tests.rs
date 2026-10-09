use efr_provider::TokenUsage;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::token_usage;

#[test]
fn the_input_is_the_sum_of_its_three_parts() {
    let usage = json!({
        "input_tokens": 120,
        "cache_creation_input_tokens": 2_000,
        "cache_read_input_tokens": 40_000,
        "cache_creation": {
            "ephemeral_5m_input_tokens": 500,
            "ephemeral_1h_input_tokens": 1_500,
        },
        "output_tokens": 900,
        "output_tokens_details": {"thinking_tokens": 600},
    });
    assert_eq!(
        token_usage(&usage),
        TokenUsage {
            input_tokens: 42_120,
            output_tokens: 900,
            cached_input_tokens: 40_000,
            reasoning_tokens: 600,
            cache_write_tokens: 2_000,
            cache_write_1h_tokens: 1_500,
        }
    );
}

#[test]
fn a_missing_or_null_count_is_zero() {
    let usage = json!({"input_tokens": 12, "cache_read_input_tokens": null, "output_tokens": 3});
    assert_eq!(
        token_usage(&usage),
        TokenUsage { input_tokens: 12, output_tokens: 3, ..TokenUsage::default() }
    );
    assert_eq!(token_usage(&json!({})), TokenUsage::default());
}

#[test]
fn the_sum_saturates() {
    let usage = json!({"input_tokens": u64::MAX, "cache_read_input_tokens": 5});
    assert_eq!(token_usage(&usage).input_tokens, u64::MAX);
}
