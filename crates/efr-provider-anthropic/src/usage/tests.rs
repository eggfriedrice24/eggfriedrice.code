use efr_provider::TokenUsage;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{StreamUsage, token_usage};

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

#[test]
fn the_five_minute_writes_are_the_writes_that_are_not_one_hour() {
    let usage = json!({
        "input_tokens": 10,
        "cache_creation_input_tokens": 700,
        "cache_creation": {"ephemeral_5m_input_tokens": 700, "ephemeral_1h_input_tokens": 0},
    });
    let usage = token_usage(&usage);
    assert_eq!((usage.cache_write_tokens, usage.cache_write_1h_tokens), (700, 0));
    assert_eq!(usage.input_tokens, 710);
}

#[test]
fn a_delta_overwrites_the_counts_that_it_sends_and_keeps_the_others() {
    let mut usage = StreamUsage::default();
    assert!(usage.is_empty());
    usage.start(&json!({
        "input_tokens": 12,
        "cache_creation_input_tokens": 3_000,
        "cache_read_input_tokens": 40_000,
        "cache_creation": {"ephemeral_5m_input_tokens": 1_000, "ephemeral_1h_input_tokens": 2_000},
        "output_tokens": 1,
    }));
    assert!(!usage.is_empty());
    // The counts are cumulative: the last value of each count wins, and a `null` or a
    // missing count keeps the value before.
    usage.update(&json!({"output_tokens": 40, "cache_read_input_tokens": null}));
    usage.update(&json!({
        "output_tokens": 310,
        "output_tokens_details": {"thinking_tokens": 200},
    }));
    assert_eq!(
        usage.token_usage(),
        TokenUsage {
            input_tokens: 43_012,
            output_tokens: 310,
            cached_input_tokens: 40_000,
            reasoning_tokens: 200,
            cache_write_tokens: 3_000,
            cache_write_1h_tokens: 2_000,
        }
    );
}

#[test]
fn a_start_replaces_the_counts_of_an_earlier_start() {
    let mut usage = StreamUsage::default();
    usage.start(&json!({"input_tokens": 5, "output_tokens": 9}));
    usage.start(&json!({"input_tokens": 7}));
    assert_eq!(usage.token_usage(), TokenUsage { input_tokens: 7, ..TokenUsage::default() });
    usage.update(&json!("not an object"));
    assert_eq!(usage.token_usage().input_tokens, 7);
}
